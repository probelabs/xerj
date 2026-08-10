//! Issue #282: port #262's ILM safety rails onto the shipped lifecycle
//! engine (#247).
//!
//! Four rails, each of which #262 carried and the merged #247 engine did
//! not, each independently a real defect:
//!
//!  1. **Detach must be honoured and survive restart.** `PUT
//!     /{index}/_settings {"index.lifecycle.name": null}` is ES's
//!     documented way to stop managing an index. Before the fix the null
//!     was silently ignored (`ilm_policy_name_from_settings` only reads
//!     strings), so the operator got `200 acknowledged` and the delete
//!     phase later destroyed the index anyway — the data-loss class from
//!     the #262 review.
//!  2. **Fail-closed action allowlist.** An ILM policy naming an action
//!     this engine cannot execute (`forcemerge`, `set_priority`, …) must
//!     be refused at PUT time with the action named, not stored with the
//!     action silently dropped (the accepted-and-ignored class, #204).
//!  3. **Delete rails.** The lifecycle delete action must never remove a
//!     dot-prefixed internal index (brains, `.kibana*`; `.ds-*` backing
//!     indices are the one exempt family), never a data stream's current
//!     write index, and never an index whose age cannot be established
//!     from its execution cursor.
//!  4. **Operator kill switch.** `GET /_ilm/status`, `POST /_ilm/start`,
//!     `POST /_ilm/stop` and `POST /{index}/_ilm/remove` must exist; while
//!     stopped, a tick must not act.
//!
//! Elasticsearch is referenced for wire semantics only; no ES code is
//! reproduced here.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

fn config_for(dir: &std::path::Path) -> xerj_common::config::Config {
    let mut config = xerj_common::config::Config::default();
    config.server.data_dir = dir.to_string_lossy().into_owned();
    config.storage.wal_sync = xerj_common::config::WalSync::Async;
    config
}

/// Build a fresh app over `dir`, returning the state handle too so tests
/// can drive `lifecycle::tick` deterministically (the background manager is
/// only spawned by the server binary, never by `Engine::new`).
fn state_over(dir: &std::path::Path) -> (axum::Router, xerj_api::state::AppState) {
    let config = config_for(dir);
    let metrics = xerj_common::metrics::Metrics::new().expect("metrics");
    let engine = xerj_engine::Engine::new(config.clone()).expect("engine");
    let state = xerj_api::state::AppState::new(config, engine, metrics);
    let app = xerj_api::router::build_es_compat_router(state.clone());
    (app, state)
}

async fn send(app: &axum::Router, req: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(req).await.expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn req_json(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(path);
    let body = match body {
        Some(v) => {
            builder = builder.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    send(app, builder.body(body).expect("request")).await
}

/// An ES ILM policy whose only phase is an immediate delete — the fastest
/// route to the data-loss outcome every rail below exists to prevent.
fn wipe_policy() -> Value {
    json!({
        "policy": {
            "phases": {
                "delete": { "min_age": "0ms", "actions": { "delete": {} } }
            }
        }
    })
}

/// A native ISM policy whose default state deletes immediately.
fn kill_now_policy() -> Value {
    json!({
        "policy": {
            "default_state": "kill",
            "states": [
                { "name": "kill", "actions": [ { "delete": {} } ], "transitions": [] }
            ]
        }
    })
}

async fn ilm_managed(app: &axum::Router, index: &str) -> bool {
    let (status, body) = req_json(app, "GET", &format!("/{index}/_ilm/explain"), None).await;
    assert_eq!(status, StatusCode::OK, "explain failed: {body}");
    body["indices"][index]["managed"].as_bool().unwrap_or(false)
}

async fn index_exists(app: &axum::Router, index: &str) -> bool {
    let (status, _) = req_json(app, "GET", &format!("/{index}"), None).await;
    status == StatusCode::OK
}

// ─────────────────────────────────────────────────────────────────────────────
// Rail 1 — detach tombstone
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn settings_null_detach_is_honoured_and_survives_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (app, state) = state_over(dir.path());

    let (status, _) = req_json(&app, "PUT", "/_ilm/policy/wipe-fast", Some(wipe_policy())).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = req_json(
        &app,
        "PUT",
        "/detach-idx",
        Some(json!({ "settings": { "index.lifecycle.name": "wipe-fast" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(ilm_managed(&app, "detach-idx").await, "attach sanity check");

    // The operator detaches. ES's documented way to stop managing an index.
    let (status, body) = req_json(
        &app,
        "PUT",
        "/detach-idx/_settings",
        Some(json!({ "index.lifecycle.name": null })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "detach refused: {body}");

    // The acknowledgement must be authoritative: the index is no longer
    // managed, and a lifecycle pass must not touch it.
    assert!(
        !ilm_managed(&app, "detach-idx").await,
        "index still managed after an acknowledged `index.lifecycle.name: null`"
    );
    xerj_engine::lifecycle::tick(&state.engine).await;
    assert!(
        index_exists(&app, "detach-idx").await,
        "lifecycle tick deleted an index the operator had detached"
    );

    // …and it must survive a restart: nothing is handed over in memory.
    drop(app);
    drop(state);
    let (app, state) = state_over(dir.path());
    assert!(
        !ilm_managed(&app, "detach-idx").await,
        "detach forgotten across restart"
    );
    xerj_engine::lifecycle::tick(&state.engine).await;
    assert!(
        index_exists(&app, "detach-idx").await,
        "restart re-attached a detached index and the delete phase fired"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Rail 2 — fail-closed action allowlist
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn unexecutable_ilm_actions_are_refused_at_put_not_skipped() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (app, _state) = state_over(dir.path());

    let (status, body) = req_json(
        &app,
        "PUT",
        "/_ilm/policy/needs-forcemerge",
        Some(json!({
            "policy": {
                "phases": {
                    "hot": {
                        "actions": {
                            "rollover": { "max_docs": 1 },
                            "forcemerge": { "max_num_segments": 1 }
                        }
                    }
                }
            }
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a policy with an action this engine cannot honour must be refused, got: {body}"
    );
    assert!(
        body.to_string().contains("forcemerge"),
        "refusal must name the action: {body}"
    );

    // Fail-closed means nothing was stored either.
    let (status, _) = req_json(&app, "GET", "/_ilm/policy/needs-forcemerge", None).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "refused policy must not be stored"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Rail 3 — delete rails
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn delete_action_never_removes_a_dot_prefixed_internal_index() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (app, state) = state_over(dir.path());

    state
        .engine
        .create_index(".ledger", xerj_common::Schema::empty())
        .expect("create dot index");
    let (status, _) = req_json(
        &app,
        "PUT",
        "/_plugins/_ism/policies/kill-now",
        Some(kill_now_policy()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = req_json(
        &app,
        "POST",
        "/_plugins/_ism/add/.ledger",
        Some(json!({ "policy_id": "kill-now" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    xerj_engine::lifecycle::tick(&state.engine).await;

    assert!(
        state.engine.get_index(".ledger").is_ok(),
        "lifecycle delete action removed a dot-prefixed internal index"
    );
    // The refusal is visible, not silent: the cursor reports failure.
    let managed = state
        .engine
        .managed_indices
        .get(".ledger")
        .expect("still managed");
    assert!(
        managed.failed,
        "refusing a delete must surface in explain, got: {}",
        managed.info_message
    );
}

#[tokio::test]
async fn delete_action_never_removes_a_data_streams_write_index() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (app, state) = state_over(dir.path());

    let (status, _) = req_json(&app, "PUT", "/_data_stream/logs-ds", None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = req_json(
        &app,
        "PUT",
        "/_plugins/_ism/policies/kill-now",
        Some(kill_now_policy()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Attach to the stream's one (and therefore write) backing index.
    let (status, _) = req_json(
        &app,
        "POST",
        "/_plugins/_ism/add/.ds-logs-ds-000001",
        Some(json!({ "policy_id": "kill-now" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    xerj_engine::lifecycle::tick(&state.engine).await;

    assert!(
        state.engine.get_index(".ds-logs-ds-000001").is_ok(),
        "lifecycle delete action removed the current write index of a data stream"
    );

    // After a rollover the old generation is no longer the write index and
    // the rail must step aside — retention on rolled-over generations is
    // the entire point of ILM on a data stream.
    let (status, _) = req_json(&app, "POST", "/logs-ds/_rollover", None).await;
    assert_eq!(status, StatusCode::OK);
    xerj_engine::lifecycle::tick(&state.engine).await;
    assert!(
        state.engine.get_index(".ds-logs-ds-000001").is_err(),
        "rolled-over backing index should now be deletable by the policy"
    );
    assert!(
        state.engine.get_index(".ds-logs-ds-000002").is_ok(),
        "the new write index must survive"
    );
}

#[tokio::test]
async fn delete_action_refuses_an_index_whose_age_cannot_be_established() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (app, state) = state_over(dir.path());

    let (status, _) = req_json(&app, "PUT", "/age-unknown-idx", Some(json!({}))).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = req_json(
        &app,
        "PUT",
        "/_plugins/_ism/policies/kill-now",
        Some(kill_now_policy()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = req_json(
        &app,
        "POST",
        "/_plugins/_ism/add/age-unknown-idx",
        Some(json!({ "policy_id": "kill-now" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Simulate a cursor whose state-entry time was lost (hand-edited or
    // corrupt `ism_managed_indices.json`): age is unestablishable.
    state
        .engine
        .managed_indices
        .get_mut("age-unknown-idx")
        .expect("managed")
        .state_entered_at_ms = 0;

    xerj_engine::lifecycle::tick(&state.engine).await;

    assert!(
        state.engine.get_index("age-unknown-idx").is_ok(),
        "lifecycle delete action removed an index whose age cannot be established"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Rail 4 — operator surface: status / start / stop / remove
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn ilm_stop_halts_execution_and_start_resumes_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (app, state) = state_over(dir.path());

    let (status, body) = req_json(&app, "GET", "/_ilm/status", None).await;
    assert_eq!(status, StatusCode::OK, "GET _ilm/status missing: {body}");
    assert_eq!(body["operation_mode"], "RUNNING");

    let (status, _) = req_json(&app, "PUT", "/stop-guard-idx", Some(json!({}))).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = req_json(
        &app,
        "PUT",
        "/_plugins/_ism/policies/kill-now",
        Some(kill_now_policy()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = req_json(
        &app,
        "POST",
        "/_plugins/_ism/add/stop-guard-idx",
        Some(json!({ "policy_id": "kill-now" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = req_json(&app, "POST", "/_ilm/stop", None).await;
    assert_eq!(status, StatusCode::OK, "POST _ilm/stop missing: {body}");
    let (_, body) = req_json(&app, "GET", "/_ilm/status", None).await;
    assert_eq!(body["operation_mode"], "STOPPED");

    xerj_engine::lifecycle::tick(&state.engine).await;
    assert!(
        state.engine.get_index("stop-guard-idx").is_ok(),
        "a tick acted while lifecycle execution was stopped"
    );

    let (status, _) = req_json(&app, "POST", "/_ilm/start", None).await;
    assert_eq!(status, StatusCode::OK);
    xerj_engine::lifecycle::tick(&state.engine).await;
    assert!(
        state.engine.get_index("stop-guard-idx").is_err(),
        "tick did not resume after POST _ilm/start"
    );
}

#[tokio::test]
async fn index_ilm_remove_detaches_and_404s_on_a_ghost() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (app, state) = state_over(dir.path());

    let (status, _) = req_json(&app, "PUT", "/remove-me", Some(json!({}))).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = req_json(
        &app,
        "PUT",
        "/_plugins/_ism/policies/kill-now",
        Some(kill_now_policy()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = req_json(
        &app,
        "POST",
        "/_plugins/_ism/add/remove-me",
        Some(json!({ "policy_id": "kill-now" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = req_json(&app, "POST", "/remove-me/_ilm/remove", None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "POST /{{index}}/_ilm/remove missing: {body}"
    );
    assert_eq!(body["has_failures"], false, "body: {body}");
    assert!(!ilm_managed(&app, "remove-me").await);

    xerj_engine::lifecycle::tick(&state.engine).await;
    assert!(
        state.engine.get_index("remove-me").is_ok(),
        "tick deleted an index after _ilm/remove detached it"
    );

    // A name that is not an index at all is a 404, not a silent success.
    let (status, _) = req_json(&app, "POST", "/ghost-idx/_ilm/remove", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
