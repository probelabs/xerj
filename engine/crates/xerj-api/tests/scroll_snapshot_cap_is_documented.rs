//! The scroll snapshot cap is a documented compatibility boundary (issue #370).
//!
//! `_search?scroll=` in XERJ is a bounded up-front snapshot, not a
//! segment-walking cursor: a query whose exact result set exceeds
//! [`SCROLL_SNAPSHOT_MAX_HITS`] is refused with a 400 rather than paged and
//! silently truncated (issue #198). That is the right behaviour — a truncated
//! export that looks complete is the worst failure mode on the
//! reindex/backup/migration path scroll exists for — but the cap was stated
//! nowhere a user reads. `grep -rn "snapshot window\|SCROLL_SNAPSHOT" docs/
//! landing/docs/ landing/llms-full.txt` returned nothing before this test.
//!
//! Scroll is what the ES ecosystem reaches for to read a whole index
//! (`elasticsearch-py`'s `helpers.scan()`, reindex and export tooling), and
//! those indices are usually larger than the cap — so the surprising case is
//! the common one. "Scroll is supported, with a 10,000-document snapshot cap;
//! use `search_after` beyond that" is a different compatibility claim from
//! "scroll is supported", and only the first one is true.
//!
//! Two halves, and both are needed:
//!
//! 1. [`scroll_over_the_cap_is_refused_and_names_the_number`] drives the real
//!    ES-compat router over a corpus one document larger than the cap and
//!    reads the enforced number back out of the 400 the server actually
//!    produced. A cap the docs describe but the server does not enforce (or
//!    enforces at a different number) fails here.
//! 2. [`published_scroll_cap_matches_the_enforced_cap`] checks every published
//!    page that states the cap against that same constant, in a marked region
//!    — the mechanism already used for the capability counts in
//!    `xerj-engine/tests/docs_capability_lists.rs`. Changing the constant now
//!    fails the build until the pages follow.
//!
//! The under-cap control in (1) exists so the test cannot pass vacuously: a
//! tree where scroll 400s unconditionally would satisfy the over-cap assertion
//! and fail the control.
//!
//! The alternative the refusal and the pages both name is not a XERJ
//! invention. Lucene documents `IndexSearcher.searchAfter` as the way to do
//! "efficient 'deep-paging' across potentially large result sets", by passing
//! the bottom result of the previous page as `after`
//! (`lucene/core/src/java/org/apache/lucene/search/IndexSearcher.java:587-607`),
//! and what it pages on is a `FieldDoc` — a hit plus its sort values
//! (`lucene/core/src/java/org/apache/lucene/search/FieldDoc.java:36-48`).
//! That is why both the 400 and the docs insist on a *unique sort key*: the
//! cursor is the sort tuple, so a non-unique one loses or repeats documents.
//!
//! Lucene (Apache-2.0) is consulted for semantics only and no code from it is
//! reproduced here. Elasticsearch likewise, and it is AGPL-3.0/SSPL-1.0/
//! Elastic-2.0 licensed.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tower::ServiceExt;
use xerj_api::es_compat::SCROLL_SNAPSHOT_MAX_HITS;

// ---------------------------------------------------------------------------
// 1. What the server enforces
// ---------------------------------------------------------------------------

/// An ES-compat app with `name` holding exactly `docs` documents, refreshed.
async fn app_with_docs(name: &str, docs: usize) -> (axum::Router, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = xerj_common::config::Config::default();
    config.server.data_dir = dir.path().to_string_lossy().into_owned();
    config.storage.wal_sync = xerj_common::config::WalSync::Async;
    let metrics = xerj_common::metrics::Metrics::new().expect("metrics");
    let engine = xerj_engine::Engine::new(config.clone()).expect("engine");
    let state = xerj_api::state::AppState::new(config, engine, metrics);

    state
        .engine
        .create_index(name, xerj_common::types::Schema::empty())
        .expect("create_index");

    let mut ndjson = String::with_capacity(docs * 32);
    for n in 0..docs {
        ndjson.push_str(&format!(
            "{{\"index\":{{\"_id\":\"{n}\"}}}}\n{{\"n\":{n}}}\n"
        ));
    }

    let app = xerj_api::router::build_es_compat_router(state.clone());
    let (status, body) = send(
        &app,
        Request::post(format!("/{name}/_bulk"))
            .header("content-type", "application/x-ndjson")
            .body(Body::from(ndjson))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "bulk seed failed: {body}");
    assert_eq!(
        body["errors"],
        json!(false),
        "bulk seed reported item errors; the corpus size this test depends on \
         would be wrong: {body}"
    );

    state
        .engine
        .get_index(name)
        .expect("get_index")
        .refresh()
        .await
        .expect("refresh");

    // The cap is compared against the *exact* total, so the seeded corpus size
    // has to be the size the engine can see. Assert it rather than assume it.
    let (status, counted) = send(
        &app,
        Request::post(format!("/{name}/_count"))
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "query": { "match_all": {} } }).to_string(),
            ))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "_count failed: {counted}");
    assert_eq!(
        counted["count"].as_u64(),
        Some(docs as u64),
        "seeded {docs} documents but the index reports {}",
        counted["count"]
    );

    (app, dir)
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

async fn open_scroll(app: &axum::Router, index: &str) -> (StatusCode, Value) {
    send(
        app,
        Request::post(format!("/{index}/_search?scroll=1m"))
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "size": 100, "query": { "match_all": {} } }).to_string(),
            ))
            .expect("request"),
    )
    .await
}

/// Pull `N` out of `... snapshot window of [N]. ...`.
///
/// Read back from the message the server produced rather than compared against
/// the constant directly: the number a user is told is the number the docs have
/// to publish, and that is this one.
fn window_in_message(reason: &str) -> usize {
    let at = reason
        .find("snapshot window of [")
        .unwrap_or_else(|| panic!("400 reason does not name the snapshot window: {reason}"))
        + "snapshot window of [".len();
    let end = reason[at..]
        .find(']')
        .unwrap_or_else(|| panic!("unterminated window in 400 reason: {reason}"))
        + at;
    reason[at..end].parse().unwrap_or_else(|e| {
        panic!(
            "window `{}` is not a number ({e}): {reason}",
            &reason[at..end]
        )
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scroll_over_the_cap_is_refused_and_names_the_number() {
    // Control: a corpus inside the cap scrolls normally. Without this, a tree
    // that 400s every scroll would pass the assertion below.
    let under_docs = 128;
    let (under, _under_dir) = app_with_docs("under_cap", under_docs).await;
    let (status, body) = open_scroll(&under, "under_cap").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a {under_docs}-document scroll is inside the cap and must succeed: {body}"
    );
    assert!(
        body["_scroll_id"].is_string(),
        "under-cap scroll returned no _scroll_id: {body}"
    );

    // One document over the cap: refused, loudly, with the number in it.
    let over = SCROLL_SNAPSHOT_MAX_HITS + 1;
    let (app, _dir) = app_with_docs("over_cap", over).await;
    let (status, body) = open_scroll(&app, "over_cap").await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a {over}-document scroll exceeds the snapshot cap and must be refused, \
         not truncated: {body}"
    );

    let reason = body["error"]["reason"]
        .as_str()
        .unwrap_or_else(|| panic!("400 has no error.reason: {body}"));
    assert!(
        reason.contains("search_after"),
        "the refusal must name the unbounded alternative, because the docs \
         checked below promise it does: {reason}"
    );
    assert_eq!(
        window_in_message(reason),
        SCROLL_SNAPSHOT_MAX_HITS,
        "the server told the user a different window than SCROLL_SNAPSHOT_MAX_HITS: {reason}"
    );
    assert!(
        reason.contains(&over.to_string()),
        "the refusal must name the offending total so the user can size the \
         gap: {reason}"
    );
}

// ---------------------------------------------------------------------------
// 2. What the docs publish
// ---------------------------------------------------------------------------

/// Repository root, derived from this crate's manifest directory
/// (`<repo>/engine/crates/xerj-api`).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("crate manifest dir must be <repo>/engine/crates/xerj-api")
        .to_path_buf()
}

/// Published pages that state the scroll cap.
///
/// A page listed here that has lost its marker is a hard failure, not a skip:
/// a silently unchecked page is how the number drifts back out of the docs.
const SCROLL_CAP_DOCS: &[&str] = &[
    "landing/docs/api-es-compat.html",
    "landing/docs/migration-from-es.html",
    "landing/llms-full.txt",
];

const SECTION: &str = "scroll-snapshot-cap";

/// A bare number is not enough. Issue #370's complaint was not "the figure is
/// missing" but "the compatibility claim is wrong without it", and the claim is
/// only complete when the page also says what happens at the ceiling (a 400)
/// and what to use instead (`search_after`). Both must appear near the marker,
/// not merely somewhere on the page.
const REQUIRED_NEARBY: &[&str] = &["search_after", "400"];

/// How much prose either side of the marker counts as "near" — a paragraph or
/// list item, generously.
const CONTEXT_BYTES: usize = 1400;

fn line_of(doc: &str, byte: usize) -> usize {
    doc[..byte].bytes().filter(|b| *b == b'\n').count() + 1
}

/// The prose around the marked region at `at` (of `len` bytes), widened to the
/// nearest char boundaries so slicing UTF-8 pages (they are full of em dashes)
/// cannot panic.
fn char_window(doc: &str, at: usize, len: usize) -> &str {
    let mut from = at.saturating_sub(CONTEXT_BYTES);
    let mut to = (at + len + CONTEXT_BYTES).min(doc.len());
    while from > 0 && !doc.is_char_boundary(from) {
        from -= 1;
    }
    while to < doc.len() && !doc.is_char_boundary(to) {
        to += 1;
    }
    &doc[from..to]
}

#[test]
fn published_scroll_cap_matches_the_enforced_cap() {
    let root = repo_root();
    let open = format!("<!-- generated:{SECTION} -->");
    let close = format!("<!-- /generated:{SECTION} -->");
    let mut seen = 0usize;

    for rel in SCROLL_CAP_DOCS {
        let doc = std::fs::read_to_string(root.join(rel))
            .unwrap_or_else(|e| panic!("cannot read {rel}: {e}"));

        let mut in_this_file = 0usize;
        for (at, _) in doc.match_indices(&open) {
            let from = at + open.len();
            let len = doc[from..].find(&close).unwrap_or_else(|| {
                panic!(
                    "{rel} opens `{open}` at line {} but never closes it",
                    line_of(&doc, at)
                )
            });
            let raw = doc[from..from + len].trim();
            // Digit grouping is allowed so the pages can read `10,000`.
            let published: usize = raw.replace(',', "").parse().unwrap_or_else(|_| {
                panic!(
                    "{rel}:{} section `{SECTION}` holds `{raw}`, which is not a \
                     number. The region is machine-checked and may hold only the \
                     cap; put the prose outside the markers.",
                    line_of(&doc, at)
                )
            });
            assert_eq!(
                published,
                SCROLL_SNAPSHOT_MAX_HITS,
                "{rel}:{} publishes a scroll snapshot cap of {published}; the \
                 server enforces {SCROLL_SNAPSHOT_MAX_HITS}",
                line_of(&doc, at)
            );

            let context = char_window(&doc, at, open.len() + len + close.len());
            for needle in REQUIRED_NEARBY {
                assert!(
                    context.contains(needle),
                    "{rel}:{} states the cap but `{needle}` does not appear within \
                     {CONTEXT_BYTES} bytes of it. A reader who hits the ceiling \
                     needs the 400 and the `search_after` way past it in the same \
                     breath as the number, or the page has told them half the \
                     compatibility story.",
                    line_of(&doc, at)
                );
            }
            in_this_file += 1;
            seen += 1;
        }

        assert!(
            in_this_file > 0,
            "{rel} carries no `{open}` region. Every page that states the scroll \
             cap is pinned to the constant; if this page genuinely no longer \
             mentions scroll, drop it from SCROLL_CAP_DOCS in this test rather \
             than leaving the number unchecked."
        );
    }

    assert!(
        seen >= SCROLL_CAP_DOCS.len(),
        "expected at least one marked cap per page in {SCROLL_CAP_DOCS:?}, found {seen}"
    );
}
