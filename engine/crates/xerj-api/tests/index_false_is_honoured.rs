//! `"index": false` on a mapping, from the outside — the last open instance of
//! the accepted-and-ignored class tracked in #204.
//!
//! Before this test's fix the option was parsed nowhere. `PUT /{index}` took
//! it, `GET /{index}/_mapping` echoed it back verbatim, and the engine went on
//! building a full inverted index for the field and answering every query
//! against it. A user who wrote `"index": false` to make a field
//! store-but-don't-search got neither half of that promise: the postings were
//! still built, and a `match` on the field still returned the document.
//!
//! The convention #204 establishes is *accepted means honoured*, so the option
//! is now honoured on both sides:
//!
//!  * a query naming a field that has neither postings nor doc values is
//!    rejected with ES's own sentence instead of being answered from
//!    `_source` (`index::unsearchable_query_field`), and
//!  * the field is dropped from the full-text index at flush and merge
//!    wherever the stored-doc scan is an equivalent fallback
//!    (`memtable::fts_excluded_fields`).
//!
//! Both halves are deliberately narrower than "reject everything / drop
//! everything", because ES 8.1 added *doc values search*: `"index": false` on
//! a keyword/numeric/date/boolean/ip field keeps its doc values and stays
//! queryable, just slower — the whole of `search/390_doc_values_search.yml`
//! depends on it. Only a field with no postings AND no doc values (a `text`
//! field, or anything with an explicit `"doc_values": false`) is unsearchable,
//! and that is exactly what ES rejects with
//! `Cannot search on field [f] since it is not indexed nor has doc values.`
//!
//! Elasticsearch is referenced for semantics only. It is AGPL-3.0/SSPL-1.0/
//! Elastic-2.0 licensed and no code from it is reproduced here.

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

async fn json_req(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
    send(
        app,
        Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("request"),
    )
    .await
}

async fn get(app: &axum::Router, path: &str) -> (StatusCode, Value) {
    send(
        app,
        Request::get(path).body(Body::empty()).expect("request"),
    )
    .await
}

async fn search(app: &axum::Router, query: Value) -> (StatusCode, Value) {
    json_req(app, "POST", "/t/_search", json!({ "query": query })).await
}

/// `note`   — text, `index: false`  → no postings, no doc values: UNSEARCHABLE.
/// `secret` — keyword, `index: false` + `doc_values: false`: UNSEARCHABLE.
/// `code`   — keyword, `index: false` → keeps doc values: still queryable.
/// `body`   — a plain indexed text field, the control.
async fn app_with_mapping() -> (axum::Router, tempfile::TempDir) {
    let (app, dir) = app().await;
    let (status, body) = json_req(
        &app,
        "PUT",
        "/t",
        json!({
            "mappings": {
                "properties": {
                    "note":   { "type": "text",    "index": false },
                    "secret": { "type": "keyword", "index": false, "doc_values": false },
                    "code":   { "type": "keyword", "index": false },
                    "body":   { "type": "text" }
                }
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create index failed: {body}");

    let (status, body) = json_req(
        &app,
        "PUT",
        "/t/_doc/1?refresh=true",
        json!({
            "note": "confidential memo about pelicans",
            "secret": "hunter2",
            "code": "AB-1234",
            "body": "confidential memo about pelicans"
        }),
    )
    .await;
    assert!(
        status.is_success(),
        "index document failed: {status} {body}"
    );
    // Force the memtable out to a segment so the FTS-exclusion half is
    // exercised on the flushed/merged path, not only in the memtable.
    let (status, _) = json_req(&app, "POST", "/t/_refresh", json!({})).await;
    assert!(status.is_success(), "refresh failed: {status}");
    (app, dir)
}

/// Accepted, echoed, and now honoured: the option survives the mapping
/// round-trip and the value is still stored and returned.
#[tokio::test]
async fn mapping_still_echoes_index_false_and_source_is_intact() {
    let (app, _dir) = app_with_mapping().await;

    let (status, mapping) = get(&app, "/t/_mapping").await;
    assert_eq!(status, StatusCode::OK, "GET _mapping failed: {mapping}");
    assert_eq!(
        mapping.pointer("/t/mappings/properties/note/index"),
        Some(&json!(false)),
        "GET _mapping must keep echoing `index: false`: {mapping}"
    );
    assert_eq!(
        mapping.pointer("/t/mappings/properties/note/type"),
        Some(&json!("text")),
        "{mapping}"
    );

    let (status, doc) = get(&app, "/t/_doc/1").await;
    assert_eq!(status, StatusCode::OK, "GET _doc failed: {doc}");
    assert_eq!(
        doc.pointer("/_source/note").and_then(Value::as_str),
        Some("confidential memo about pelicans"),
        "a non-indexed field is still stored and returned: {doc}"
    );
}

/// THE REGRESSION. Before the fix this returned 200 with one hit, because the
/// field kept its postings and the stored-doc scan matched it from `_source`.
#[tokio::test]
async fn querying_a_field_with_no_postings_and_no_doc_values_is_rejected() {
    let (app, _dir) = app_with_mapping().await;

    for query in [
        json!({ "match": { "note": "pelicans" } }),
        json!({ "term":  { "note": "pelicans" } }),
        json!({ "match_phrase": { "note": "confidential memo" } }),
        json!({ "prefix": { "note": { "value": "pel" } } }),
        json!({ "wildcard": { "note": { "value": "pel*" } } }),
        // Nested inside a bool filter — the walk must recurse, not just peek
        // at the top-level clause.
        json!({ "bool": { "filter": [ { "term": { "note": "pelicans" } } ] } }),
        // …and inside must_not, where a silent non-match would invert the
        // result set rather than merely shrink it.
        json!({ "bool": { "must_not": [ { "match": { "note": "pelicans" } } ] } }),
    ] {
        let (status, body) = search(&app, query.clone()).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "query {query} must be rejected, got {status}: {body}"
        );
        assert_eq!(
            body.pointer("/error/type").and_then(Value::as_str),
            Some("search_phase_execution_exception"),
            "{body}"
        );
        assert_eq!(
            body.pointer("/error/root_cause/0/type")
                .and_then(Value::as_str),
            Some("query_shard_exception"),
            "{body}"
        );
        let reason = body
            .pointer("/error/root_cause/0/reason")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            reason.contains(
                "Cannot search on field [note] since it is not indexed nor has doc values."
            ),
            "reason must carry ES's sentence, got {reason:?}: {body}"
        );
    }

    // An explicit `doc_values: false` reaches the same verdict on a keyword.
    let (status, body) = search(&app, json!({ "term": { "secret": "hunter2" } })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let reason = body
        .pointer("/error/root_cause/0/reason")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        reason.contains("Cannot search on field [secret]"),
        "got {reason:?}: {body}"
    );
}

/// `index: false` is not "unqueryable" on its own. ES 8.1's doc-values search
/// still answers term/terms/range on a keyword that kept its doc values, and so
/// must XERJ — otherwise honouring the option would break
/// `search/390_doc_values_search.yml`.
#[tokio::test]
async fn index_false_with_doc_values_stays_queryable() {
    let (app, _dir) = app_with_mapping().await;

    for query in [
        json!({ "term":  { "code": "AB-1234" } }),
        json!({ "terms": { "code": ["AB-1234", "ZZ-0000"] } }),
    ] {
        let (status, body) = search(&app, query.clone()).await;
        assert_eq!(status, StatusCode::OK, "query {query} → {status}: {body}");
        assert_eq!(
            body.pointer("/hits/total/value").and_then(Value::as_u64),
            Some(1),
            "doc-values search must still find the doc for {query}: {body}"
        );
    }
}

/// No collateral damage: an ordinary indexed field is unaffected, and `exists`
/// on an unsearchable field is 0 hits rather than an error (ES leaves such a
/// field out of `_field_names`; it does not fail the query).
#[tokio::test]
async fn indexed_fields_and_exists_are_unaffected() {
    let (app, _dir) = app_with_mapping().await;

    let (status, body) = search(&app, json!({ "match": { "body": "pelicans" } })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body.pointer("/hits/total/value").and_then(Value::as_u64),
        Some(1),
        "the indexed control field must still match: {body}"
    );

    let (status, body) = search(&app, json!({ "exists": { "field": "note" } })).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "`exists` on an unsearchable field is not an error in ES: {body}"
    );
    assert_eq!(
        body.pointer("/hits/total/value").and_then(Value::as_u64),
        Some(0),
        "{body}"
    );
}

/// The check runs AFTER alias resolution, so an alias cannot be used to reach
/// a field the mapping made unsearchable.
#[tokio::test]
async fn an_alias_to_an_unsearchable_field_is_rejected_too() {
    let (app, _dir) = app().await;
    let (status, body) = json_req(
        &app,
        "PUT",
        "/t",
        json!({
            "mappings": {
                "properties": {
                    "note":       { "type": "text",  "index": false },
                    "note_alias": { "type": "alias", "path": "note" },
                    "body":       { "type": "text" }
                }
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = json_req(
        &app,
        "PUT",
        "/t/_doc/1?refresh=true",
        json!({ "note": "zzquagga", "body": "ordinary text" }),
    )
    .await;
    assert!(status.is_success(), "{status} {body}");

    let (status, body) = search(&app, json!({ "match": { "note_alias": "zzquagga" } })).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "an alias must inherit its target's searchability: {body}"
    );
    let reason = body
        .pointer("/error/root_cause/0/reason")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        reason.contains("Cannot search on field [note]"),
        "the error names the resolved target, as ES does: {reason:?}"
    );
}
