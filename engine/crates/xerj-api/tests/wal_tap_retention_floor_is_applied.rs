//! Issue #320 — `PUT /_xerj/wal_tap` must not acknowledge a retention floor
//! it does not apply.
//!
//! `wal_tap.min_retained_generations` is the knob whose entire purpose is
//! keeping WAL entries alive while a replication target is down. It reached
//! `WalWriter` through one path only: `store_config_from`, which reads
//! `Engine.config` — an `Arc<Config>` written once at boot from `xerj.toml`
//! and never mutated. `WalTap::set_config` wrote the tap's own `RwLock` and
//! the state file and nothing else.
//!
//! So a runtime `PUT` reached no writer at all: not the indices already open,
//! and — because the boot `Config` is the *file's* value, not the persisted
//! runtime overlay — not after a restart either. The endpoint answered
//! `200 {"acknowledged":true,"persisted":true,...}` plus a `warning` saying it
//! would take effect on restart. An operator who raised the floor to survive a
//! target outage, saw the `200`, restarted, and still lost WAL entries had been
//! told a falsehood by the API.
//!
//! These tests drive the whole chain over HTTP and then read the floor back off
//! the live `WalWriter`s.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

async fn request(app: &axum::Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

fn app_on(dir: &std::path::Path) -> (axum::Router, xerj_api::state::AppState) {
    let mut config = xerj_common::config::Config::default();
    config.server.data_dir = dir.to_string_lossy().into_owned();
    config.storage.wal_sync = xerj_common::config::WalSync::Async;
    let metrics = xerj_common::metrics::Metrics::new().expect("metrics");
    let engine = xerj_engine::Engine::new(config.clone()).expect("engine");
    let state = xerj_api::state::AppState::new(config, engine, metrics);
    let app = xerj_api::router::build_es_compat_router(state.clone());
    (app, state)
}

/// The floor the API reports must be the floor the WAL writers hold — for the
/// indices that are already open, for indices created afterwards, and across a
/// restart.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_retention_floor_set_over_http_reaches_the_live_wal_writers() {
    let dir = tempfile::tempdir().expect("data dir");
    let (app, state) = app_on(dir.path());

    let (status, _) = request(&app, "PUT", "/already-open", json!({})).await;
    assert_eq!(status, StatusCode::OK, "index create");

    let before = state
        .engine
        .get_index("already-open")
        .expect("index")
        .wal_min_retained_generations();
    // Non-empty first: every `all(...)` below is vacuously true on an empty
    // vec, which would make this whole test pass while reading nothing.
    let shards = before.len();
    assert!(
        shards > 0,
        "the index must have WAL shards to assert about, got {before:?}"
    );
    assert!(
        before.iter().all(|&n| n == 0),
        "the default floor is 0, got {before:?}"
    );

    let (status, body) = request(
        &app,
        "PUT",
        "/_xerj/wal_tap",
        json!({"min_retained_generations": 3}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "PUT /_xerj/wal_tap: {body}");
    assert_eq!(body["wal_tap"]["min_retained_generations"], 3);
    assert_eq!(
        body["retention_floor_applied_to_indices"], 1,
        "the response must report how many live WAL writers the floor reached: {body}"
    );
    assert!(
        body.get("warning").is_none(),
        "there is nothing left to warn about — it applied: {body}"
    );

    // The claim, checked against the writers themselves.
    let after = state
        .engine
        .get_index("already-open")
        .expect("index")
        .wal_min_retained_generations();
    assert_eq!(after.len(), shards, "shard count must not have changed");
    assert!(
        after.iter().all(|&n| n == 3),
        "an index open BEFORE the PUT must hold the new floor on every WAL shard, got {after:?}"
    );

    // An index created after the PUT opens with it too, rather than inheriting
    // the boot file's 0.
    let (status, _) = request(&app, "PUT", "/created-after", json!({})).await;
    assert_eq!(status, StatusCode::OK, "index create");
    let created_after = state
        .engine
        .get_index("created-after")
        .expect("index")
        .wal_min_retained_generations();
    assert_eq!(
        created_after.len(),
        shards,
        "shard count must not have changed"
    );
    assert!(
        created_after.iter().all(|&n| n == 3),
        "an index created AFTER the PUT must open with the floor, got {created_after:?}"
    );

    // And a restart. The config FILE still says 0 — nothing wrote to
    // `xerj.toml` — so every part of this depends on the persisted runtime
    // overlay reaching the storage layer.
    drop(app);
    drop(state);
    let (_app2, state2) = app_on(dir.path());
    assert_eq!(
        state2.engine.wal_tap.config().min_retained_generations,
        3,
        "the persisted runtime floor must survive the restart"
    );
    // The value every `IndexStore` is SEEDED with at open, before anything
    // re-applies it. `Engine::new` folds the tap's effective configuration
    // into the boot `Config` for exactly this: `store_config_from` reads it
    // inside `IndexStore::open`, which replays the WAL, and a store must not
    // spend its replay holding a floor the operator has already changed.
    assert_eq!(
        state2.engine.config().wal_tap.min_retained_generations,
        3,
        "the boot Config a store opens with must carry the effective floor, not the file's"
    );
    let reopened = state2
        .engine
        .get_index("already-open")
        .expect("index")
        .wal_min_retained_generations();
    assert_eq!(reopened.len(), shards, "shard count must not have changed");
    assert!(
        reopened.iter().all(|&n| n == 3),
        "after a restart the reopened index must hold the persisted floor, not the config \
         file's — this is the exact case the old `warning` claimed to cover and did not, \
         got {reopened:?}"
    );
}

/// `DELETE /_xerj/wal_tap` reverts to the config file, and that has to reach
/// the writers as well — otherwise clearing an expensive floor would leave the
/// disk cost behind with nothing in the API to show for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deleting_the_runtime_config_releases_the_retention_floor() {
    let dir = tempfile::tempdir().expect("data dir");
    let (app, state) = app_on(dir.path());
    let (status, _) = request(&app, "PUT", "/logs", json!({})).await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = request(
        &app,
        "PUT",
        "/_xerj/wal_tap",
        json!({"min_retained_generations": 5}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let held = state
        .engine
        .get_index("logs")
        .expect("index")
        .wal_min_retained_generations();
    let shards = held.len();
    assert!(shards > 0, "no WAL shards to assert about: {held:?}");
    assert!(
        held.iter().all(|&n| n == 5),
        "the floor must be on every shard before DELETE can be shown to clear it, got {held:?}"
    );

    let (status, body) = request(&app, "DELETE", "/_xerj/wal_tap", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["source"], "config_file");
    assert_eq!(body["wal_tap"]["min_retained_generations"], 0);
    let released = state
        .engine
        .get_index("logs")
        .expect("index")
        .wal_min_retained_generations();
    assert_eq!(released.len(), shards, "shard count must not have changed");
    assert!(
        released.iter().all(|&n| n == 0),
        "reverting to the config file must release the floor on the live writers too, \
         got {released:?}"
    );
}

/// `max_retry_backoff_secs` is range-checked like `poll_interval_ms`, not
/// merely `!= 0`. `WalTap::arm_backoff` saturates so no value can overflow it,
/// but a multi-millennium cap is a tap that stops retrying, not a backoff.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_absurd_retry_cap_is_refused_at_the_api() {
    let dir = tempfile::tempdir().expect("data dir");
    let (app, _state) = app_on(dir.path());

    for bad in [0u64, 86_401, u64::MAX] {
        let (status, body) = request(
            &app,
            "PUT",
            "/_xerj/wal_tap",
            json!({"max_retry_backoff_secs": bad}),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "max_retry_backoff_secs={bad} must be refused, got {status} {body}"
        );
    }

    let (status, body) = request(
        &app,
        "PUT",
        "/_xerj/wal_tap",
        json!({"max_retry_backoff_secs": 86_400}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "the bound itself is valid: {body}");
    assert_eq!(body["wal_tap"]["max_retry_backoff_secs"], 86_400);
}
