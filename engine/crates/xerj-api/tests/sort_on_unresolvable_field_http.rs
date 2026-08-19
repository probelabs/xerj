//! Issue #437, from the outside: sorting on a field the engine cannot
//! resolve must be rejected with ES's real error shape, on every HTTP entry
//! point that accepts a `sort` clause — not just `_search`.
//!
//! Issue #402's request-level denylist only ever reached `search_impl`, and
//! its own review caught `_msearch`/`_search/template`/`_async_search`
//! answering 200 on the same "unresolvable" field name it rejected on
//! `_search`. This suite proves the engine-level fix (a single gate inside
//! `search_inner`, which every one of these handlers funnels through) closes
//! that gap structurally rather than needing each endpoint wired in by hand.

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

async fn send(app: &axum::Router, req: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(req).await.expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

async fn seed(app: &axum::Router, index: &str, n: u64) {
    let mut ndjson = String::new();
    for i in 0..n {
        ndjson.push_str(&format!(
            "{{\"index\":{{\"_id\":\"{i}\"}}}}\n{{\"n\":{i}}}\n"
        ));
    }
    let (st, body) = send(
        app,
        Request::builder()
            .method("POST")
            .uri(format!("/{index}/_bulk"))
            .header("content-type", "application/x-ndjson")
            .body(Body::from(ndjson))
            .unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "bulk {index}: {body}");
    let (st, body) = send(
        app,
        Request::builder()
            .method("POST")
            .uri(format!("/{index}/_refresh"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "refresh {index}: {body}");
}

/// The real ES envelope: `search_phase_execution_exception`/400 wrapping
/// `query_shard_exception` in `root_cause`, reason naming the field.
fn assert_no_mapping_found_error(status: StatusCode, body: &Value, field: &str) {
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "sort on unresolvable field [{field}] must be a 400, not silently answered: {body}"
    );
    assert_eq!(
        body["error"]["type"], "search_phase_execution_exception",
        "wrong outer error type for [{field}]: {body}"
    );
    assert_eq!(
        body["error"]["root_cause"][0]["type"], "query_shard_exception",
        "wrong root_cause type for [{field}]: {body}"
    );
    let reason = body["error"]["reason"].as_str().unwrap_or_default();
    assert!(
        reason.contains(&format!("No mapping found for [{field}]")),
        "reason for [{field}] doesn't name the field: {reason}"
    );
    assert_eq!(
        body["status"], 400,
        "wrong top-level status for [{field}]: {body}"
    );
}

#[tokio::test]
async fn search_rejects_sort_on_every_unresolvable_meta_field_and_a_typo() {
    let (app, _dir) = app().await;
    seed(&app, "census", 4).await;

    const FIELDS: &[&str] = &[
        "_source",
        "_size",
        "_doc_count",
        "_field_names",
        "_meta",
        "_tier",
        "_nested",
        "_nested_path",
        "_feature",
        "_parent",
        "_matched_queries",
        "not_a_field_at_all",
    ];
    for field in FIELDS {
        let (status, body) = send(
            &app,
            Request::builder()
                .method("POST")
                .uri("/census/_search")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"query": {"match_all": {}}, "sort": [{*field: "asc"}]}).to_string(),
                ))
                .unwrap(),
        )
        .await;
        assert_no_mapping_found_error(status, &body, field);
    }
}

/// The actual user-visible harm the issue is about: `search_after` paging
/// on an unresolvable field silently stops at page one instead of erroring.
/// A mapped field (control) must keep walking the whole corpus.
#[tokio::test]
async fn search_after_is_rejected_up_front_instead_of_stranded_at_page_one() {
    let (app, _dir) = app().await;
    seed(&app, "strand", 6).await;

    // Control: search_after on a real field walks the whole corpus.
    let (status, page1) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri("/strand/_search")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"query": {"match_all": {}}, "size": 2, "sort": [{"n": "asc"}, {"_id": "asc"}]})
                    .to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "control page 1: {page1}");
    let hits = page1["hits"]["hits"].as_array().expect("hits array");
    assert_eq!(hits.len(), 2, "control page 1 must return a full page");
    let cursor = hits.last().unwrap()["sort"].clone();
    let (status, page2) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri("/strand/_search")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "query": {"match_all": {}}, "size": 2,
                    "sort": [{"n": "asc"}, {"_id": "asc"}],
                    "search_after": cursor,
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "control page 2: {page2}");
    assert_eq!(
        page2["hits"]["hits"].as_array().map(Vec::len),
        Some(2),
        "control search_after must advance past page 1, not repeat it: {page2}"
    );

    // The bug: search_after on an unresolvable field, pre-fix, answered 200
    // with every hit's sort value null and page 2 identical to page 1
    // (stranded). Post-fix it must be refused up front.
    let (status, body) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri("/strand/_search")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"query": {"match_all": {}}, "size": 2, "sort": [{"_tier": "asc"}]})
                    .to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_no_mapping_found_error(status, &body, "_tier");
}

/// #402's own review caught this exact gap: the denylist reached `_search`
/// but not `_msearch`. Prove the single engine-level gate closes it without
/// per-endpoint wiring.
#[tokio::test]
async fn msearch_rejects_sort_on_an_unresolvable_field_too() {
    let (app, _dir) = app().await;
    seed(&app, "mscensus", 4).await;

    let ndjson = format!(
        "{}\n{}\n",
        json!({"index": "mscensus"}),
        json!({"query": {"match_all": {}}, "sort": [{"_meta": "asc"}]})
    );
    let (status, body) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri("/_msearch")
            .header("content-type", "application/x-ndjson")
            .body(Body::from(ndjson))
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "msearch envelope itself is always 200: {body}"
    );
    let responses = body["responses"].as_array().expect("responses array");
    assert_eq!(responses.len(), 1);
    assert_no_mapping_found_error(StatusCode::BAD_REQUEST, &responses[0], "_meta");
}

/// Same gap, `_search/template`.
#[tokio::test]
async fn search_template_rejects_sort_on_an_unresolvable_field_too() {
    let (app, _dir) = app().await;
    seed(&app, "tplcensus", 4).await;

    let (status, body) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri("/tplcensus/_search/template")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "source": {"query": {"match_all": {}}, "sort": [{"_field_names": "asc"}]},
                    "params": {},
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_no_mapping_found_error(status, &body, "_field_names");
}

/// Positive control: a `.keyword` multi-field and a nested/object property
/// must keep sorting correctly — the schema check uses the same recursive
/// dotted-path resolution `compute_sort_values` already relies on for
/// `.keyword`, not a flat exact-match lookup that would false-reject both.
#[tokio::test]
async fn sort_on_a_keyword_multi_field_and_a_nested_property_still_works() {
    let (app, _dir) = app().await;
    let (st, body) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri("/nested_ok/_bulk")
            .header("content-type", "application/x-ndjson")
            .body(Body::from(
                "{\"index\":{\"_id\":\"1\"}}\n\
                 {\"title\":\"b\",\"user\":{\"name\":\"z\"}}\n\
                 {\"index\":{\"_id\":\"2\"}}\n\
                 {\"title\":\"a\",\"user\":{\"name\":\"y\"}}\n",
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "bulk: {body}");
    let (st, body) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri("/nested_ok/_refresh")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "refresh: {body}");

    for field in ["title.keyword", "user.name"] {
        let (status, body) = send(
            &app,
            Request::builder()
                .method("POST")
                .uri("/nested_ok/_search")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"query": {"match_all": {}}, "sort": [{field: "asc"}]}).to_string(),
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "sort on [{field}] must work: {body}"
        );
        assert_eq!(body["hits"]["hits"].as_array().map(Vec::len), Some(2));
    }
}

/// The `#437` gate must NOT reject a sort on an unmapped field that carries
/// ES's `unmapped_type` escape hatch: real Elasticsearch answers 200 and
/// treats every document as missing, and XERJ's own memory-list endpoint
/// issues exactly `{"stored_at":{"order":"desc","unmapped_type":"long"}}` as a
/// guard for namespaces created before `stored_at` existed. Rejecting it (the
/// #504 review's blocking finding) would 400 those requests. Both directions
/// are pinned here so the exemption cannot silently regress in either sense.
#[tokio::test]
async fn sort_on_an_unmapped_field_with_unmapped_type_is_not_rejected() {
    let (app, _dir) = app().await;
    seed(&app, "umt", 4).await; // docs carry only `n`; `stored_at` is unmapped

    // With unmapped_type: ES treats every doc as missing — 200, not an error.
    let (status, body) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri("/umt/_search")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "query": {"match_all": {}},
                    "sort": [{"stored_at": {"order": "desc", "unmapped_type": "long"}}]
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "sort on an unmapped field carrying unmapped_type must be answered (ES \
         treats it as all-missing), not rejected: {body}"
    );
    assert_eq!(
        body["hits"]["total"]["value"], 4,
        "all four docs must come back, sorted as missing: {body}"
    );

    // Without unmapped_type the same field is still rejected (the #437 fix).
    let (status, body) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri("/umt/_search")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"query": {"match_all": {}}, "sort": [{"stored_at": "desc"}]}).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_no_mapping_found_error(status, &body, "stored_at");
}
