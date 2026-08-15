//! REST surface for the single-node WAL tap (issue #320).
//!
//! ```text
//! GET    /_xerj/wal_tap          current configuration (never the credential)
//! PUT    /_xerj/wal_tap          merge-patch it, durably, no restart
//! DELETE /_xerj/wal_tap          drop the runtime overlay, revert to the file
//! GET    /_xerj/wal_tap/_stats   per-index cursors, lag, counters, last error
//! ```
//!
//! This is a native (`_xerj`-namespaced) surface, not an Elasticsearch one:
//! there is no ES endpoint that means "push my WAL somewhere", and pretending
//! `_ccr/*` did would be exactly the silent-stub behaviour those handlers were
//! made to `501` to avoid.
//!
//! ## Why every route here is superuser-only
//!
//! `PUT /_xerj/wal_tap` is a whole-node export primitive: `target_url` plus
//! `indices: ["*"]` streams every index on this node to a host of the
//! caller's choosing, with an operator-supplied `Authorization` header
//! attached — exfiltration and authenticated SSRF in one request. Nothing
//! here is index-scoped, so there is no per-index decision to make, and that
//! is the same reasoning `authz.rs` already applies to snapshot and restore.
//! The reads are covered for the same reason: `GET /_xerj/wal_tap` echoes
//! `target_url` and `_stats` enumerates the node's whole index inventory.
//! Enforced in [`crate::authz`] on the `_xerj` first segment, not here, so a
//! second `_xerj` route cannot be added without inheriting it.

use axum::{
    extract::State,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

use xerj_common::config::WalTapConfig;
use xerj_engine::wal_tap::WalTap;

use crate::{extract::OptionalJson, state::AppState};

/// The configuration as the API reports it.
///
/// `target_auth` is deliberately absent: it is a bearer credential for another
/// cluster, and an endpoint that reads it back turns "can call the admin API"
/// into "holds the target's credential". `target_auth_configured` is the
/// operator-visible part.
///
/// `target_url` is **redacted**, and that is not belt and braces — it closes
/// the hole the omission above left wide open. `https://user:pass@host:9200`
/// is an ordinary URL, and `reqwest` turns its userinfo into a `Basic`
/// `Authorization` header, so an operator who put the credential there was
/// configuring `target_auth` by another spelling — and this endpoint would
/// have handed it straight back. [`WalTapConfig::check_target_url`] refuses
/// userinfo on the way in; this makes sure a URL that arrived some other way
/// (an older state file, a hand-edited config) still cannot be read back out.
fn config_body(config: &WalTapConfig) -> Value {
    json!({
        "enabled": config.enabled,
        "target_url": config.redacted_target_url(),
        "target_auth_configured": !config.target_auth.is_empty(),
        "indices": config.indices,
        "poll_interval_ms": config.poll_interval_ms,
        "max_batch_docs": config.max_batch_docs,
        "max_batch_bytes": config.max_batch_bytes,
        "request_timeout_secs": config.request_timeout_secs,
        "max_retry_backoff_secs": config.max_retry_backoff_secs,
        "min_retained_generations": config.min_retained_generations,
    })
}

/// `GET /_xerj/wal_tap`
pub async fn get_wal_tap(State(state): State<AppState>) -> Response {
    let mut body = config_body(&state.engine.wal_tap.config());
    if let Some(obj) = body.as_object_mut() {
        // Which of the two sources this configuration came from is the
        // question an operator asks after a restart, and the one the tap used
        // to answer wrongly by silently reverting to the file.
        obj.insert(
            "source".into(),
            json!(if state.engine.wal_tap.has_runtime_config() {
                "runtime"
            } else {
                "config_file"
            }),
        );
    }
    Json(body).into_response()
}

/// The mutable half. Every field optional — an absent field keeps its current
/// value, so an operator can flip `enabled` without restating the allowlist.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WalTapPatch {
    pub enabled: Option<bool>,
    pub target_url: Option<String>,
    /// Write-only. `""` clears the stored credential.
    pub target_auth: Option<String>,
    pub indices: Option<Vec<String>>,
    pub poll_interval_ms: Option<u64>,
    pub max_batch_docs: Option<usize>,
    pub max_batch_bytes: Option<usize>,
    pub request_timeout_secs: Option<u64>,
    pub max_retry_backoff_secs: Option<u64>,
    pub min_retained_generations: Option<u64>,
}

fn bad_request(reason: &str) -> Response {
    (
        axum::http::StatusCode::BAD_REQUEST,
        Json(json!({
            "error": {"type": "illegal_argument_exception", "reason": reason},
            "status": 400,
        })),
    )
        .into_response()
}

/// `PUT /_xerj/wal_tap`
///
/// Takes effect on the next poll. Cursors are keyed by index name and are
/// untouched here, so adding an index to the allowlist starts it "from now"
/// while an index that was already running resumes exactly where it was —
/// removing and re-adding one does not re-ship what it already shipped.
pub async fn put_wal_tap(
    State(state): State<AppState>,
    OptionalJson(body): OptionalJson<WalTapPatch>,
) -> Response {
    let patch = body.unwrap_or_default();
    let mut config = state.engine.wal_tap.config();

    if let Some(v) = patch.enabled {
        config.enabled = v;
    }
    if let Some(v) = patch.target_url {
        let trimmed = v.trim().to_string();
        // Shape AND credential check. `https://user:pass@host` passed the old
        // scheme-only test, and `reqwest` would have turned that userinfo into
        // a `Basic Authorization` header — a credential smuggled into the one
        // field this API echoes back (and logs at boot).
        if let Err(reason) = WalTapConfig::check_target_url(&trimmed) {
            return bad_request(&reason);
        }
        config.target_url = trimmed;
    }
    if let Some(v) = patch.target_auth {
        config.target_auth = v;
    }
    if let Some(v) = patch.indices {
        // A pattern that can only ever match system indices is a mistake worth
        // reporting rather than silently dropping: the tap refuses `.xerj*`
        // unconditionally, so such a pattern would ship nothing forever and
        // look like a broken tap.
        if let Some(bad) = v.iter().find(|p| p.starts_with(".xerj")) {
            return bad_request(&format!(
                "wal_tap.indices pattern {bad:?} only matches system indices, which are \
                 never shipped"
            ));
        }
        config.indices = v;
    }
    if let Some(v) = patch.poll_interval_ms {
        if !(50..=60_000).contains(&v) {
            return bad_request("wal_tap.poll_interval_ms must be between 50 and 60000");
        }
        config.poll_interval_ms = v;
    }
    if let Some(v) = patch.max_batch_docs {
        if v == 0 {
            return bad_request("wal_tap.max_batch_docs must be at least 1");
        }
        config.max_batch_docs = v;
    }
    if let Some(v) = patch.max_batch_bytes {
        if v == 0 {
            return bad_request("wal_tap.max_batch_bytes must be at least 1");
        }
        config.max_batch_bytes = v;
    }
    if let Some(v) = patch.request_timeout_secs {
        if v == 0 {
            return bad_request("wal_tap.request_timeout_secs must be at least 1");
        }
        config.request_timeout_secs = v;
    }
    if let Some(v) = patch.max_retry_backoff_secs {
        if v == 0 {
            return bad_request("wal_tap.max_retry_backoff_secs must be at least 1");
        }
        config.max_retry_backoff_secs = v;
    }
    if let Some(v) = patch.min_retained_generations {
        // Bounded on purpose: this knob costs `v * storage.wal_max_size_mb`
        // per WAL shard per index whether or not a tap is running, so an
        // operator typing an extra digit must not be able to fill the disk
        // quietly.
        if v > 64 {
            return bad_request(
                "wal_tap.min_retained_generations must be at most 64: it holds that many \
                 rotated WAL files per shard per index, costing up to \
                 n * storage.wal_max_size_mb of disk each",
            );
        }
        config.min_retained_generations = v;
    }

    if config.enabled && config.target_url.is_empty() {
        return bad_request("wal_tap.enabled requires a target_url");
    }

    let body = config_body(&config);
    // `min_retained_generations` reaches `WalWriter` through
    // `IndexStoreConfig` when a store is opened, so a runtime change to it
    // applies to indices opened after this point (and to every index after a
    // restart, which this patch now survives). Said plainly in the response
    // rather than left for an operator to discover.
    let retention_takes_effect_on_restart =
        patch.min_retained_generations.is_some() && !state.engine.index_name_list().is_empty();
    state.engine.wal_tap.set_config(config);
    let mut resp = json!({"acknowledged": true, "wal_tap": body, "persisted": true});
    if retention_takes_effect_on_restart {
        resp["warning"] = json!(
            "min_retained_generations is read when an index store is opened: already-open \
             indices keep the previous retention floor until this node restarts"
        );
    }
    Json(resp).into_response()
}

/// `DELETE /_xerj/wal_tap`
///
/// Drop the persisted runtime configuration and revert to the config file.
///
/// The counterpart to `PUT` now that a `PUT` is durable: without this, one
/// API call would permanently shadow `xerj.toml` with no way back short of
/// deleting a file in the data directory. Modelled on `_cluster/settings`,
/// where writing `null` clears a persistent setting.
pub async fn delete_wal_tap(State(state): State<AppState>) -> Response {
    // The engine's boot `Config` is the file's value: the tap took a clone of
    // it at construction and only then layered the persisted runtime config
    // on top, so this is genuinely "what xerj.toml says".
    let file_config = state.engine.config().wal_tap.clone();
    state.engine.wal_tap.clear_runtime_config(file_config);
    Json(json!({
        "acknowledged": true,
        "wal_tap": config_body(&state.engine.wal_tap.config()),
        "source": "config_file",
    }))
    .into_response()
}

/// `GET /_xerj/wal_tap/_stats`
///
/// The honesty surface. `gaps > 0` means entries were pruned out of the WAL
/// before the tap could ship them and are gone; `lag_seq` is how far behind
/// the index's own sequence counter the tap is; `last_error` is why it stopped
/// advancing; `healthy` is false when either of the two states that used to
/// be indistinguishable from working is in effect — sends failing, or a
/// target answering 200 while rejecting every action inside it.
///
/// Superuser-only ([`crate::authz`]): this enumerates every index on the node
/// that the allowlist matches, and it echoes `target_url`. It applies no
/// principal filter of its own precisely because it is never reached by a
/// principal that would need one.
pub async fn wal_tap_stats(State(state): State<AppState>) -> Response {
    let tap = &state.engine.wal_tap;
    let config = tap.config();
    let stats = tap.stats();
    let cursors = tap.cursors();

    let mut indices = serde_json::Map::new();
    let mut all_healthy = true;
    for name in state.engine.index_name_list() {
        if !WalTap::ships(&config, &name) {
            continue;
        }
        let stat = stats.get(&name).cloned().unwrap_or_default();
        // `current_seq_no` is the NEXT seq to be handed out, so the highest
        // reserved one is that minus 1.
        let head = state
            .engine
            .get_index(&name)
            .map(|i| i.current_seq_no().saturating_sub(1))
            .unwrap_or(0);
        let mut body = serde_json::to_value(&stat).unwrap_or_else(|_| json!({}));
        if let Some(obj) = body.as_object_mut() {
            obj.insert(
                "lag_seq".into(),
                json!(head.saturating_sub(stat.last_shipped_seq)),
            );
            // One boolean an operator can alert on, so "is replication
            // working?" does not require correlating five counters.
            obj.insert("healthy".into(), json!(stat.healthy()));
            obj.insert(
                "cursors".into(),
                serde_json::to_value(cursors.get(&name).cloned().unwrap_or_default())
                    .unwrap_or_else(|_| json!({})),
            );
        }
        if !stat.healthy() {
            all_healthy = false;
        }
        indices.insert(name, body);
    }

    Json(json!({
        "enabled": config.enabled,
        // Redacted for the same reason as in `config_body`: `_stats` is a
        // second read path onto the same string.
        "target_url": config.redacted_target_url(),
        "ticks": tap.ticks(),
        "healthy": all_healthy,
        "indices": indices,
    }))
    .into_response()
}
