//! REST surface for the single-node WAL tap (issue #320).
//!
//! ```text
//! GET  /_xerj/wal_tap          current configuration (never the credential)
//! PUT  /_xerj/wal_tap          merge-patch it, no restart
//! GET  /_xerj/wal_tap/_stats   per-index cursors, lag, counters, last error
//! ```
//!
//! This is a native (`_xerj`-namespaced) surface, not an Elasticsearch one:
//! there is no ES endpoint that means "push my WAL somewhere", and pretending
//! `_ccr/*` did would be exactly the silent-stub behaviour those handlers were
//! made to `501` to avoid.
//!
//! Everything here is cluster-scoped, so it authorises like any other
//! `/_*` route (`authz::classify` → `Target::Cluster`).

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
fn config_body(config: &WalTapConfig) -> Value {
    json!({
        "enabled": config.enabled,
        "target_url": config.target_url,
        "target_auth_configured": !config.target_auth.is_empty(),
        "indices": config.indices,
        "poll_interval_ms": config.poll_interval_ms,
        "max_batch_docs": config.max_batch_docs,
        "max_batch_bytes": config.max_batch_bytes,
        "request_timeout_secs": config.request_timeout_secs,
        "max_retry_backoff_secs": config.max_retry_backoff_secs,
    })
}

/// `GET /_xerj/wal_tap`
pub async fn get_wal_tap(State(state): State<AppState>) -> Response {
    Json(config_body(&state.engine.wal_tap.config())).into_response()
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
        if !trimmed.is_empty()
            && !(trimmed.starts_with("http://") || trimmed.starts_with("https://"))
        {
            return bad_request(
                "wal_tap.target_url must be an absolute http:// or https:// URL, e.g. \
                 \"https://central:9200\"",
            );
        }
        config.target_url = trimmed;
    }
    if let Some(v) = patch.target_auth {
        config.target_auth = v;
    }
    if let Some(v) = patch.indices {
        // A pattern that can only ever match system indices is a mistake worth
        // reporting rather than silently dropping: the tap refuses `.xerj-*`
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

    if config.enabled && config.target_url.is_empty() {
        return bad_request("wal_tap.enabled requires a target_url");
    }

    let body = config_body(&config);
    state.engine.wal_tap.set_config(config);
    Json(json!({"acknowledged": true, "wal_tap": body})).into_response()
}

/// `GET /_xerj/wal_tap/_stats`
///
/// The honesty surface. `gaps > 0` means entries were pruned out of the WAL
/// before the tap could ship them and are gone; `lag_seq` is how far behind
/// the index's own sequence counter the tap is; `last_error` is why it stopped
/// advancing.
pub async fn wal_tap_stats(State(state): State<AppState>) -> Response {
    let tap = &state.engine.wal_tap;
    let config = tap.config();
    let stats = tap.stats();
    let cursors = tap.cursors();

    let mut indices = serde_json::Map::new();
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
            obj.insert(
                "cursors".into(),
                serde_json::to_value(cursors.get(&name).cloned().unwrap_or_default())
                    .unwrap_or_else(|_| json!({})),
            );
        }
        indices.insert(name, body);
    }

    Json(json!({
        "enabled": config.enabled,
        "target_url": config.target_url,
        "ticks": tap.ticks(),
        "indices": indices,
    }))
    .into_response()
}
