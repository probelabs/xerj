//! Issue #440: `_seq_no`/`_version` on a scroll page must come from the SAME
//! read that produced `_source`, not a later pass over the version map.
//!
//! Two repair attempts on PR #431 were refuted by live reproduction:
//!   1. resolve live at render time — window is the whole scroll lifetime.
//!   2. resolve once at scroll-open time, from a context-level index lookup
//!      — narrower, but still a SECOND read, so a write landing between the
//!      `_source` read and the `_seq_no` read pairs a stale body with a live
//!      sequence number. Since `_seq_no` is fed back as `if_seq_no`, that is
//!      a silent lost update, not a cosmetic staleness.
//!
//! The fix threads `seq_no`/`version` onto `xerj_query::executor::Hit`
//! itself, populated in the engine at the exact point `_source` is read.
//! These tests exercise that guarantee from the outside, through the real
//! HTTP routes — the in-module unit tests only ever injected a synthetic
//! `ScrollContext` with fabricated `Hit`s, which is exactly why the original
//! bug (and PR #431's two subsequent regressions) had no version-map entry
//! to expose it.

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

async fn create_index(app: &axum::Router, index: &str) {
    let (st, body) = json_req(
        app,
        "PUT",
        &format!("/{index}"),
        json!({"mappings": {"properties": {"v": {"type": "long"}}}}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "create {index}: {body}");
}

async fn refresh(app: &axum::Router, index: &str) {
    let (st, body) = json_req(app, "POST", &format!("/{index}/_refresh"), Value::Null).await;
    assert_eq!(st, StatusCode::OK, "refresh {index}: {body}");
}

async fn bulk_index(app: &axum::Router, index: &str, ids: impl Iterator<Item = usize>) {
    let mut bulk = String::new();
    for i in ids {
        bulk.push_str(&format!("{}\n", json!({"index": {"_id": i.to_string()}})));
        bulk.push_str(&format!("{}\n", json!({"v": i})));
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
    assert_eq!(response.status(), StatusCode::OK);
}

/// Multi-index scroll, ids colliding across indices (the reindex/CDC shape
/// #440 measured against) — every hit's `_seq_no`/`_version` must agree with
/// what a plain `_search` on the hit's OWN index reports for the same doc.
///
/// PR #431's own repair attempt #2 (resolve once at scroll-open, from a
/// context-level index) failed exactly this scenario: 4 of 8 hits carried
/// values read out of the OTHER index's version map. This test's structure
/// — colliding ids, differing per-index write counts so `_seq_no` is not
/// merely a page ordinal — reproduces the shape that caught it.
#[tokio::test]
async fn multi_index_scroll_seq_no_matches_plain_search_per_index() {
    let (app, _dir) = app().await;
    create_index(&app, "sq_a").await;
    create_index(&app, "sq_b").await;

    // Different write counts per index so `_seq_no` is not simply the page
    // ordinal (which would let a wrong-index lookup pass unnoticed if the
    // two indices' sequence numbers happened to line up).
    bulk_index(&app, "sq_a", 0..4).await;
    bulk_index(&app, "sq_a", 0..4).await; // re-index sq_a's docs once more
    bulk_index(&app, "sq_b", 0..4).await;
    refresh(&app, "sq_a").await;
    refresh(&app, "sq_b").await;

    let (st, first) = json_req(
        &app,
        "POST",
        "/sq_a,sq_b/_search?scroll=2m",
        json!({
            "query": {"match_all": {}},
            "size": 3,
            "seq_no_primary_term": true,
            "version": true,
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "scroll start: {first}");

    let mut sid = first["_scroll_id"].as_str().map(str::to_string);
    let mut page = first;
    let mut scrolled: Vec<(String, String, i64, i64)> = Vec::new();

    for _ in 0..16 {
        let hits = page["hits"]["hits"].as_array().cloned().unwrap_or_default();
        if hits.is_empty() {
            break;
        }
        for h in &hits {
            scrolled.push((
                h["_index"].as_str().unwrap_or_default().to_string(),
                h["_id"].as_str().unwrap_or_default().to_string(),
                h["_seq_no"]
                    .as_i64()
                    .expect("scrolled hit must carry _seq_no"),
                h["_version"]
                    .as_i64()
                    .expect("scrolled hit must carry _version"),
            ));
        }
        let Some(id) = sid.clone() else { break };
        let (st, next) = json_req(
            &app,
            "POST",
            "/_search/scroll",
            json!({"scroll": "2m", "scroll_id": id}),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "scroll continuation: {next}");
        sid = next["_scroll_id"].as_str().map(str::to_string).or(sid);
        page = next;
    }
    assert_eq!(
        scrolled.len(),
        8,
        "must have scrolled every hit across both indices"
    );

    // Ground truth: a direct GET on the hit's OWN index — the same
    // resolver (`lookup_seq_no`/`lookup_version`) `_mget`/plain `_search`
    // use, isolated from any query-matching ambiguity.
    for (idx, id, scrolled_seq, scrolled_version) in &scrolled {
        let (st, truth) = json_req(&app, "GET", &format!("/{idx}/_doc/{id}"), Value::Null).await;
        assert_eq!(st, StatusCode::OK, "truth GET {idx}/{id}: {truth}");
        assert_eq!(
            truth["_seq_no"].as_i64(),
            Some(*scrolled_seq),
            "scrolled _seq_no for {idx}/{id} does not match a direct GET on its own index \
             — a hit is carrying another index's version-map entry (#440)"
        );
        assert_eq!(
            truth["_version"].as_i64(),
            Some(*scrolled_version),
            "scrolled _version for {idx}/{id} does not match a direct GET on its own index"
        );
    }
}

/// The direct #440 repro: a document already captured in the scroll
/// snapshot is updated AFTER the scroll opens but BEFORE its continuation
/// page is fetched. The scrolled page must report the SNAPSHOT's
/// `_seq_no`/`_source` pair — not a live `_seq_no` beside a stale
/// `_source`, which is exactly the torn read that lets a caller's
/// `if_seq_no` CAS silently overwrite the concurrent update.
#[tokio::test]
async fn scroll_continuation_reports_the_snapshot_pair_not_a_torn_one() {
    let (app, _dir) = app().await;
    create_index(&app, "torn").await;
    bulk_index(&app, "torn", 0..4).await;
    refresh(&app, "torn").await;

    // page_size 1 forces every hit onto its own continuation page.
    let (st, first) = json_req(
        &app,
        "POST",
        "/torn/_search?scroll=5m",
        json!({
            "query": {"match_all": {}},
            "size": 1,
            "sort": [{"v": "asc"}],
            "seq_no_primary_term": true,
            "version": true,
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "scroll start: {first}");
    let sid = first["_scroll_id"].as_str().expect("scroll id").to_string();
    let page1_id = first["hits"]["hits"][0]["_id"]
        .as_str()
        .unwrap()
        .to_string();
    let page1_seq = first["hits"]["hits"][0]["_seq_no"]
        .as_i64()
        .expect("_seq_no on page 1");

    // Concurrent write AFTER the scroll snapshot: bump doc 1's `v` and its
    // real seq_no/version. If the scroll re-derives against the LIVE
    // index, page 2 (doc "1") would report this write's seq_no next to
    // page 1's already-returned doc (id "0") — but the direct exposure is
    // doc "1" itself, captured below.
    let (st, upd) = json_req(&app, "PUT", "/torn/_doc/1", json!({"v": 999})).await;
    assert_eq!(st, StatusCode::OK, "concurrent update: {upd}");
    let live_seq_after_update = upd["_seq_no"].as_i64().expect("_seq_no on update");

    // Continue the scroll to the page holding doc "1" (sorted by v asc,
    // doc "1" is the second hit — id "0" was v=0, id "1" was v=1 pre-update).
    let (st, page2) = json_req(
        &app,
        "POST",
        "/_search/scroll",
        json!({"scroll": "5m", "scroll_id": sid}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "scroll continuation: {page2}");
    let hit = &page2["hits"]["hits"][0];
    assert_eq!(
        hit["_id"], "1",
        "page 2 must be the doc updated concurrently"
    );
    let scrolled_seq = hit["_seq_no"].as_i64().expect("_seq_no on page 2");
    let scrolled_source_v = hit["_source"]["v"].as_i64();

    assert_ne!(
        scrolled_seq, live_seq_after_update,
        "scrolled page reported the LIVE post-update _seq_no ({live_seq_after_update}) — \
         a caller round-tripping this as if_seq_no would CAS successfully over the \
         concurrent write, a silent lost update (#440)"
    );
    assert_eq!(
        scrolled_source_v,
        Some(1),
        "scrolled _source must be the SNAPSHOT body (v=1, pre-update), not the live one — \
         found {scrolled_source_v:?}"
    );
    assert_ne!(
        page1_seq, scrolled_seq,
        "sanity: two different docs must not share a _seq_no"
    );
    let _ = page1_id;
}

/// #440's "adjacent, smaller, separate" gap: `disable_sequence_numbers`
/// applies its `-2`/`0` sentinel in `search_impl`'s response body but
/// `scroll_page_response` had no equivalent at all — a scroll continuation
/// page for such an index reported whatever `_seq_no` the snapshotted hit
/// carried (or, pre-fix, a hardcoded `0`) instead of ES's `-2` sentinel.
#[tokio::test]
async fn scroll_continuation_honours_disable_sequence_numbers() {
    let (app, _dir) = app().await;
    let (st, body) = json_req(
        &app,
        "PUT",
        "/noseq",
        json!({
            "settings": {"index": {"disable_sequence_numbers": true}},
            "mappings": {"properties": {"v": {"type": "long"}}}
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "create noseq: {body}");
    bulk_index(&app, "noseq", 0..3).await;
    refresh(&app, "noseq").await;

    let (st, first) = json_req(
        &app,
        "POST",
        "/noseq/_search?scroll=2m",
        json!({"query": {"match_all": {}}, "size": 1, "sort": [{"v": "asc"}]}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "scroll start: {first}");
    let sid = first["_scroll_id"].as_str().expect("scroll id").to_string();

    let (st, page2) = json_req(
        &app,
        "POST",
        "/_search/scroll",
        json!({"scroll": "2m", "scroll_id": sid}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "scroll continuation: {page2}");
    let hit = &page2["hits"]["hits"][0];
    assert_eq!(
        hit["_seq_no"].as_i64(),
        Some(-2),
        "continuation page for a disable_sequence_numbers index must carry the -2 \
         sentinel, got: {hit}"
    );
    assert_eq!(
        hit["_primary_term"].as_i64(),
        Some(0),
        "continuation page for a disable_sequence_numbers index must carry primary_term 0"
    );
}
