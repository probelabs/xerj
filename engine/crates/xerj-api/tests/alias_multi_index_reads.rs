//! Issue #433: a multi-index alias answered as an alias over ONE member on
//! every read path.
//!
//! Three indices behind one alias, 40 documents each. `GET /_alias/kb` listed
//! all three, and `_count`, `_search` and a scroll through the alias all
//! answered 40 — from the first member, with no error and a count that agreed
//! with the short result. Not a keyed consumer silently keeping a fraction:
//! every consumer got a fraction.
//!
//! The cause was three independent copies of index-selector resolution.
//! `resolve_index_selector` expanded aliases; `search_impl`'s own resolver only
//! looked for `_all` and `*`; `count_docs` gated its whole multi-index branch on
//! the same two plus a comma or a star. A plain alias fell through both.
//!
//! These go through the real HTTP routes because that is the only place the
//! three resolvers meet.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
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

async fn call(app: &axum::Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
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
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

const PER_INDEX: usize = 40;

async fn seed(app: &axum::Router) {
    for member in ["kb-a", "kb-b", "kb-c"] {
        let (st, _) = call(
            app,
            "PUT",
            &format!("/{member}"),
            json!({"mappings": {"properties": {"v": {"type": "long"}, "home": {"type": "keyword"}}}}),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "create {member}");

        let mut bulk = String::new();
        for i in 0..PER_INDEX {
            bulk.push_str(&format!("{}\n", json!({"index": {"_id": i.to_string()}})));
            bulk.push_str(&format!("{}\n", json!({"v": i, "home": member})));
        }
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/{member}/_bulk"))
                    .header("content-type", "application/x-ndjson")
                    .body(Body::from(bulk))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "bulk {member}");
        let (st, _) = call(app, "POST", &format!("/{member}/_refresh"), Value::Null).await;
        assert_eq!(st, StatusCode::OK);
    }
    let (st, _) = call(
        app,
        "POST",
        "/_aliases",
        json!({"actions": [
            {"add": {"index": "kb-a", "alias": "kb"}},
            {"add": {"index": "kb-b", "alias": "kb"}},
            {"add": {"index": "kb-c", "alias": "kb"}},
            {"add": {"index": "kb-a", "alias": "solo"}}
        ]}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "create aliases");
}

/// `_count` and `_search` must see every member, and a one-member alias must
/// keep answering for exactly that member.
#[tokio::test]
async fn every_read_path_through_a_multi_index_alias_sees_every_member() {
    let (app, _dir) = app().await;
    seed(&app).await;
    let all = (PER_INDEX * 3) as u64;

    let (_, count) = call(&app, "GET", "/kb/_count", Value::Null).await;
    assert_eq!(
        count["count"].as_u64(),
        Some(all),
        "_count through the alias must cover all three members: {count}"
    );

    let (_, search) = call(
        &app,
        "POST",
        "/kb/_search",
        json!({"query": {"match_all": {}}, "size": 0}),
    )
    .await;
    assert_eq!(
        search["hits"]["total"]["value"].as_u64(),
        Some(all),
        "_search through the alias must cover all three members: {search}"
    );

    // The single-member case is the one a narrowing fix would break.
    let (_, solo) = call(&app, "GET", "/solo/_count", Value::Null).await;
    assert_eq!(
        solo["count"].as_u64(),
        Some(PER_INDEX as u64),
        "an alias over one index still answers for that index: {solo}"
    );
    let (_, concrete) = call(&app, "GET", "/kb-a/_count", Value::Null).await;
    assert_eq!(concrete["count"].as_u64(), Some(PER_INDEX as u64));
}

/// A scroll through the alias must walk every document AND report the index
/// each hit truly came from — `(_index, _id)` is the key reindex, migration,
/// backup and CDC consumers use, and ids collide across the members.
#[tokio::test]
async fn a_scroll_through_an_alias_walks_every_member_and_labels_each_hit() {
    let (app, _dir) = app().await;
    seed(&app).await;

    let (st, mut page) = call(
        &app,
        "POST",
        "/kb/_search?scroll=2m",
        json!({"query": {"match_all": {}}, "size": 50}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "scroll start: {page}");
    let mut sid = page["_scroll_id"].as_str().map(str::to_string);
    let mut hits: Vec<(String, String, String)> = Vec::new();

    for _ in 0..16 {
        let page_hits = page["hits"]["hits"].as_array().cloned().unwrap_or_default();
        if page_hits.is_empty() {
            break;
        }
        for h in &page_hits {
            hits.push((
                h["_index"].as_str().unwrap_or_default().to_string(),
                h["_id"].as_str().unwrap_or_default().to_string(),
                h["_source"]["home"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            ));
        }
        let Some(id) = sid.clone() else { break };
        let (st, next) = call(
            &app,
            "POST",
            "/_search/scroll",
            json!({"scroll": "2m", "scroll_id": id}),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "continuation: {next}");
        sid = next["_scroll_id"].as_str().map(str::to_string).or(sid);
        page = next;
    }

    assert_eq!(hits.len(), PER_INDEX * 3, "every member must be walked");

    let mislabelled: Vec<_> = hits
        .iter()
        .filter(|(reported, _, home)| reported != home)
        .take(5)
        .collect();
    assert!(
        mislabelled.is_empty(),
        "hits must report the index they came from, not the alias: {mislabelled:?}"
    );

    let distinct: std::collections::HashSet<(&str, &str)> = hits
        .iter()
        .map(|(i, id, _)| (i.as_str(), id.as_str()))
        .collect();
    assert_eq!(
        distinct.len(),
        PER_INDEX * 3,
        "(_index, _id) must be distinct — ids collide across the members"
    );
}
