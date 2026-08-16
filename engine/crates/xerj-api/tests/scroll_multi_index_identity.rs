//! Issue #414, from the outside: on a multi-index scroll every hit must report
//! the index it actually came from, so `(_index, _id)` stays distinct.
//!
//! Ids collide across indices as a matter of course — each index numbers its
//! own documents — so `(_index, _id)` is the only key that separates them. When
//! the scroll stamps one name onto every hit, a consumer keyed on that pair
//! silently keeps a fraction of the corpus. Scroll is what reindex, migration,
//! backup and CDC use, which is where losing half a corpus hurts most.
//!
//! The in-module unit test for this covers the *read* site only: it hand-builds
//! a `ScrollContext` and calls the continuation. That leaves both *creation*
//! sites unguarded — an independent verification of PR #424 showed the defect
//! could be reintroduced at either one with all 216 tests still passing. This
//! test goes through the real HTTP routes instead, so the creation sites are
//! actually exercised:
//!
//!   * `POST /{a,b}/_search?scroll=` — `search_impl`
//!   * `POST /_search/scroll`        — the continuation
//!
//! Each document carries `_source.home` = the index it was written to, so the
//! assertion compares the reported `_index` against ground truth per hit rather
//! than against a count.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::collections::HashSet;
use tower::ServiceExt;

async fn app() -> (axum::Router, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = xerj_common::config::Config::default();
    config.server.data_dir = dir.path().to_string_lossy().into_owned();
    config.storage.wal_sync = xerj_common::config::WalSync::Async;
    let metrics = xerj_common::metrics::Metrics::new().expect("metrics");
    let engine = xerj_engine::Engine::new(config.clone()).expect("engine");
    let state = xerj_api::state::AppState::new(config, engine, metrics);
    (xerj_api::router::build_es_compat_router(state), dir)
}

async fn json_req(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(path);
    let body = if body.is_null() {
        Body::empty()
    } else {
        req = req.header("content-type", "application/json");
        Body::from(body.to_string())
    };
    let response = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, v)
}

/// Documents per index. Ids are `0..N` in BOTH, so every id collides.
const N: usize = 120;

async fn seed(app: &axum::Router, index: &str) {
    let (st, _) = json_req(
        app,
        "PUT",
        &format!("/{index}"),
        json!({"mappings": {"properties": {"v": {"type": "long"}, "home": {"type": "keyword"}}}}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "create {index}");

    let mut bulk = String::new();
    for i in 0..N {
        bulk.push_str(&format!("{}\n", json!({"index": {"_id": i.to_string()}})));
        bulk.push_str(&format!("{}\n", json!({"v": i, "home": index})));
    }
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/{index}/_bulk"))
                .header("content-type", "application/x-ndjson")
                .body(Body::from(bulk))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "bulk {index}");
    let (st, _) = json_req(app, "POST", &format!("/{index}/_refresh"), Value::Null).await;
    assert_eq!(st, StatusCode::OK, "refresh {index}");
}

/// Walk a scroll to exhaustion, returning `(reported_index, id, true_home)` per
/// hit. `page_size` deliberately does not divide `N`, so a page straddles the
/// boundary between the two indices — the case an off-by-one in the pairing
/// would otherwise slip through.
async fn walk(app: &axum::Router, spec: &str, page_size: usize) -> Vec<(String, String, String)> {
    let (st, first) = json_req(
        app,
        "POST",
        &format!("/{spec}/_search?scroll=2m"),
        json!({"query": {"match_all": {}}, "size": page_size}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "scroll start: {first}");

    let mut sid = first["_scroll_id"].as_str().map(str::to_string);
    let mut page = first;
    let mut out = Vec::new();

    for _ in 0..64 {
        let hits = page["hits"]["hits"].as_array().cloned().unwrap_or_default();
        if hits.is_empty() {
            break;
        }
        for h in &hits {
            out.push((
                h["_index"].as_str().unwrap_or_default().to_string(),
                h["_id"].as_str().unwrap_or_default().to_string(),
                h["_source"]["home"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            ));
        }
        let Some(id) = sid.clone() else { break };
        let (st, next) = json_req(
            app,
            "POST",
            "/_search/scroll",
            json!({"scroll": "2m", "scroll_id": id}),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "scroll continuation: {next}");
        sid = next["_scroll_id"].as_str().map(str::to_string).or(sid);
        page = next;
    }
    out
}

#[tokio::test]
async fn multi_index_scroll_reports_the_index_each_hit_came_from() {
    let (app, _dir) = app().await;
    seed(&app, "mi_a").await;
    seed(&app, "mi_b").await;

    let hits = walk(&app, "mi_a,mi_b", 50).await;

    assert_eq!(
        hits.len(),
        N * 2,
        "scroll must return every document across both indices"
    );

    // Ground truth per hit: `home` was written into `_source` at index time.
    let mislabelled: Vec<_> = hits
        .iter()
        .filter(|(reported, _, home)| reported != home)
        .take(5)
        .collect();
    assert!(
        mislabelled.is_empty(),
        "hits reported an _index they did not come from (first {}): {:?}",
        mislabelled.len(),
        mislabelled
    );

    // The property that actually matters to a consumer.
    let distinct: HashSet<(&str, &str)> = hits
        .iter()
        .map(|(idx, id, _)| (idx.as_str(), id.as_str()))
        .collect();
    assert_eq!(
        distinct.len(),
        N * 2,
        "(_index, _id) is not distinct — {} hits collapsed to {} keys, which is \
         how a keyed consumer silently loses documents (#414)",
        hits.len(),
        distinct.len()
    );
}

/// The single-index case must keep behaving, and is the overwhelmingly common
/// one: the type change behind #414 touches every scroll, not just multi-index
/// ones.
#[tokio::test]
async fn single_index_scroll_is_unchanged() {
    let (app, _dir) = app().await;
    seed(&app, "solo").await;

    let hits = walk(&app, "solo", 50).await;
    assert_eq!(hits.len(), N);
    assert!(
        hits.iter().all(|(reported, _, home)| reported == home),
        "single-index scroll mislabelled a hit"
    );
    let distinct: HashSet<(&str, &str)> = hits
        .iter()
        .map(|(idx, id, _)| (idx.as_str(), id.as_str()))
        .collect();
    assert_eq!(distinct.len(), N);
}
