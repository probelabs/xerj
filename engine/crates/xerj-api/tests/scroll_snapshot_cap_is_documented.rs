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
//! Three parts, and all three are needed:
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
//! 3. [`published_search_after_recipe_walks_the_whole_corpus`] executes the
//!    escape hatch the pages publish. It parses the `search_after` transcript
//!    out of `landing/docs/api-es-compat.html`, seeds the corpus that
//!    transcript names, replays the printed request bodies verbatim against
//!    the real router, and checks that following the page's own stop rule
//!    ("until a page comes back empty") really does collect every document,
//!    over the page count the transcript claims.
//!
//! Part 3 exists because part 2 is not enough, and the gap is not academic:
//! the transcript first published here paged with `search_after: ["999"]`
//! against 11,450 numeric-looking `_id`s. `_id` is a keyword and sorts
//! LEXICOGRAPHICALLY, so `"1000" < "999"` and `"999"` sits near the *end* of
//! that corpus — the printed walk terminated after 1,010 of 11,450 documents,
//! with a 200 and an empty page, which is exactly the stop condition the page
//! states. Every published number was right, the cap was pinned, the walk
//! silently lost 91% of the export — on the page whose entire thesis is that
//! XERJ will not hand back a truncated export that looks complete (issue
//! #198). A recipe is only documented if it runs.
//!
//! The under-cap control in (1) exists so the test cannot pass vacuously: a
//! tree where scroll 400s unconditionally would satisfy the over-cap assertion
//! and fail the control. Part (3) cannot pass vacuously either: it asserts the
//! full set of seeded ids, so a walk that returns nothing, or stops early, or
//! repeats a page, fails.
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
//! Lucene is equally explicit that a text sort key is ordered by bytes, not by
//! meaning: a binary/`SortField.Type#STRING` sort orders documents "by the
//! unsigned byte order of their sort key (`BytesRef#compareTo`)"
//! (`lucene/core/src/java/org/apache/lucene/search/BinarySortField.java:52-61`),
//! and getting *numeric* order out of that byte comparison takes a deliberate
//! encoding — `NumericUtils.intToSortableBytes` exists to "encode an integer
//! value such that unsigned byte order comparison is consistent with
//! `Integer#compare(int, int)`", flipping the sign bit to do it
//! (`lucene/core/src/java/org/apache/lucene/util/NumericUtils.java:178-188`).
//! Digits stored as text get no such encoding, which is the whole of why
//! `"1000" < "999"` and why the first transcript was wrong.
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

// ---------------------------------------------------------------------------
// 3. What the docs tell the user to *do*
// ---------------------------------------------------------------------------

/// The page carrying the `search_after` escape hatch, and the anchor of the
/// section that carries it.
const RECIPE_PAGE: &str = "landing/docs/api-es-compat.html";
const RECIPE_ANCHOR: &str = "id=\"scroll-cap\"";

/// The shell transcript in the scroll-cap section: the first
/// `<pre class="code">` block after the section's anchor.
fn scroll_cap_transcript(page: &str) -> &str {
    const OPEN: &str = "<pre class=\"code\">";
    const CLOSE: &str = "</pre>";
    let anchor = page
        .find(RECIPE_ANCHOR)
        .unwrap_or_else(|| panic!("{RECIPE_PAGE} has no `{RECIPE_ANCHOR}` section"));
    let rest = &page[anchor..];
    let from = rest
        .find(OPEN)
        .unwrap_or_else(|| panic!("{RECIPE_PAGE}: no `{OPEN}` transcript after {RECIPE_ANCHOR}"))
        + OPEN.len();
    let len = rest[from..]
        .find(CLOSE)
        .unwrap_or_else(|| panic!("{RECIPE_PAGE}: transcript after {RECIPE_ANCHOR} is unclosed"));
    &rest[from..from + len]
}

/// Every `-d '…'` request body in a shell transcript, in printed order.
///
/// Only a literal `-d '` opens a capture, so apostrophes in the surrounding
/// prose ("the last hit's `sort` value") and the quoted `-H` headers are not
/// mistaken for payload delimiters.
fn curl_payloads(block: &str) -> Vec<String> {
    const FLAG: &str = "-d '";
    let mut out = Vec::new();
    let mut rest = block;
    while let Some(at) = rest.find(FLAG) {
        let from = at + FLAG.len();
        let len = rest[from..]
            .find('\'')
            .unwrap_or_else(|| panic!("{RECIPE_PAGE}: unterminated `{FLAG}` payload"));
        out.push(rest[from..from + len].to_string());
        rest = &rest[from + len + 1..];
    }
    out
}

/// The transcript's own summary line: the `# → …` comment stating what the walk
/// above it produces, up to the end of that line.
///
/// Anchoring on the arrow rather than on "… pages" keeps the claim readings
/// below unambiguous no matter what the surrounding prose says.
fn claim_line(block: &str) -> &str {
    const ARROW: &str = "# → ";
    let at = block
        .find(ARROW)
        .unwrap_or_else(|| panic!("{RECIPE_PAGE}: transcript states no `{ARROW}` outcome"));
    let rest = &block[at + ARROW.len()..];
    rest.split('\n').next().unwrap_or(rest)
}

/// The digits (with optional `,` grouping) immediately preceding `needle`.
///
/// Lets the test read the transcript's own claims — "12 pages", "all 11,450
/// documents" — instead of restating them, so a transcript edited to claim
/// something else is checked against the new claim.
fn number_before(hay: &str, needle: &str) -> usize {
    let at = hay
        .find(needle)
        .unwrap_or_else(|| panic!("{RECIPE_PAGE}: transcript does not say `{needle}`: {hay}"));
    let head = hay[..at].trim_end();
    let start = head
        .rfind(|c: char| !c.is_ascii_digit() && c != ',')
        .map_or(0, |i| i + 1);
    head[start..].replace(',', "").parse().unwrap_or_else(|e| {
        panic!(
            "{RECIPE_PAGE}: `{}` before `{needle}` is not a number ({e})",
            &head[start..]
        )
    })
}

/// The number inside the first `open`…`]` after `open` (e.g. `too large: [`).
fn bracketed_after(hay: &str, open: &str) -> usize {
    let at = hay
        .find(open)
        .unwrap_or_else(|| panic!("{RECIPE_PAGE}: transcript does not contain `{open}`"))
        + open.len();
    let end = hay[at..]
        .find(']')
        .unwrap_or_else(|| panic!("{RECIPE_PAGE}: unterminated `{open}` bracket"))
        + at;
    hay[at..end]
        .replace(',', "")
        .parse()
        .unwrap_or_else(|e| panic!("{RECIPE_PAGE}: `{}` is not a number ({e})", &hay[at..end]))
}

async fn search(app: &axum::Router, index: &str, body: String) -> (StatusCode, Value) {
    send(
        app,
        Request::post(format!("/{index}/_search"))
            .header("content-type", "application/json")
            .body(Body::from(body))
            .expect("request"),
    )
    .await
}

/// The published `search_after` recipe, executed.
///
/// Issue #370 asked for the cap to be documented. The documentation is only
/// worth anything if the way *past* the cap, which is the one thing a reader
/// blocked by it will copy, actually works — so this replays it: the printed
/// bodies verbatim, the printed cursor verbatim, the page's own stop rule, and
/// the corpus the transcript itself names.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn published_search_after_recipe_walks_the_whole_corpus() {
    let page = std::fs::read_to_string(repo_root().join(RECIPE_PAGE))
        .unwrap_or_else(|e| panic!("cannot read {RECIPE_PAGE}: {e}"));
    let transcript = scroll_cap_transcript(&page);

    // The transcript's own premise and its own claimed outcome.
    let corpus = bracketed_after(transcript, "too large: [");
    let claim = claim_line(transcript);
    let claimed_pages = number_before(claim, " pages");
    let claimed_docs = number_before(claim, " documents");
    assert!(
        corpus > SCROLL_SNAPSHOT_MAX_HITS,
        "the transcript's premise is a corpus over the cap, but it names {corpus} \
         against a cap of {SCROLL_SNAPSHOT_MAX_HITS}"
    );
    assert_eq!(
        claimed_docs, corpus,
        "the transcript refuses a scroll over {corpus} documents but then claims the \
         `search_after` walk returns {claimed_docs}; the two halves must describe the \
         same index or the recipe is not the escape hatch for the refusal above it"
    );

    // The printed request bodies. The refused-scroll example has no `sort`;
    // the two `search_after` pages do.
    let paged: Vec<String> = curl_payloads(transcript)
        .into_iter()
        .filter(|p| p.contains("\"sort\""))
        .collect();
    assert_eq!(
        paged.len(),
        2,
        "expected exactly two sorted request bodies in the transcript (the first page \
         and one continuation); found {}: {paged:?}",
        paged.len()
    );
    let (first_body, next_body) = (&paged[0], &paged[1]);
    let first: Value = serde_json::from_str(first_body)
        .unwrap_or_else(|e| panic!("printed first page body is not JSON ({e}): {first_body}"));
    let next: Value = serde_json::from_str(next_body)
        .unwrap_or_else(|e| panic!("printed continuation body is not JSON ({e}): {next_body}"));
    assert!(
        first.get("search_after").is_none(),
        "the first printed page must open the walk, not continue one: {first_body}"
    );
    let printed_cursor = next
        .get("search_after")
        .unwrap_or_else(|| {
            panic!("the printed continuation carries no `search_after`: {next_body}")
        })
        .clone();
    assert_eq!(
        first.get("sort"),
        next.get("sort"),
        "the two printed pages sort differently, so the second cannot continue the \
         first: {first_body} then {next_body}"
    );

    // The corpus the transcript describes: `corpus` documents whose `_id`s are
    // the decimal integers, which is what makes the lexicographic trap live.
    let (app, _dir) = app_with_docs("logs", corpus).await;

    // Page 1, exactly as printed.
    let (status, page1) = search(&app, "logs", first_body.clone()).await;
    assert_eq!(status, StatusCode::OK, "printed first page failed: {page1}");
    let hits = page1["hits"]["hits"]
        .as_array()
        .unwrap_or_else(|| panic!("printed first page returned no hits array: {page1}"))
        .clone();
    assert!(
        !hits.is_empty(),
        "printed first page returned zero hits over a {corpus}-document index: {page1}"
    );

    // The cursor the reader is told to use is the previous page's last `sort`.
    // If the printed one is not that value, the printed walk starts somewhere
    // else in the sort order — which is how a keyset export silently drops
    // documents while still returning 200s.
    let real_cursor = hits
        .last()
        .and_then(|h| h.get("sort"))
        .unwrap_or_else(|| panic!("hits carry no `sort` values to page on: {page1}"))
        .clone();
    assert_eq!(
        printed_cursor,
        real_cursor,
        "the transcript continues with search_after={printed_cursor} but page 1 of the \
         corpus it describes ends at search_after={real_cursor} (last _id {}). `_id` is \
         a keyword and sorts lexicographically, so a cursor guessed from the page size \
         lands in the wrong place and the walk ends early with a 200 and an empty page. \
         Publish the cursor the response actually returns.",
        hits.last().map(|h| h["_id"].clone()).unwrap_or(Value::Null)
    );

    // Now walk it: printed continuation first, then the page's own rule —
    // "feed the last hit's `sort` value back as `search_after` until a page
    // comes back empty".
    let mut seen: std::collections::HashSet<String> = hits
        .iter()
        .map(|h| h["_id"].as_str().expect("_id").to_string())
        .collect();
    let mut returned = hits.len();
    let mut pages = 1usize;
    let mut body = next_body.clone();
    let ceiling = claimed_pages * 4 + 16;
    loop {
        let (status, resp) = search(&app, "logs", body.clone()).await;
        assert_eq!(status, StatusCode::OK, "page {} failed: {resp}", pages + 1);
        let page_hits = resp["hits"]["hits"]
            .as_array()
            .unwrap_or_else(|| panic!("page {} returned no hits array: {resp}", pages + 1))
            .clone();
        if page_hits.is_empty() {
            break;
        }
        pages += 1;
        returned += page_hits.len();
        for hit in &page_hits {
            seen.insert(hit["_id"].as_str().expect("_id").to_string());
        }
        assert!(
            pages <= ceiling,
            "the published walk did not terminate within {ceiling} pages; the \
             transcript claims {claimed_pages}"
        );
        let cursor = page_hits
            .last()
            .and_then(|h| h.get("sort"))
            .unwrap_or_else(|| panic!("page {pages} carries no `sort` to continue from: {resp}"))
            .clone();
        let mut cont = next.clone();
        cont["search_after"] = cursor;
        body = cont.to_string();
    }

    assert_eq!(
        returned,
        seen.len(),
        "the published walk returned {returned} hits but only {} distinct ids — a \
         keyset walk that repeats documents is as wrong as one that skips them",
        seen.len()
    );
    let missing: Vec<String> = (0..corpus)
        .map(|n| n.to_string())
        .filter(|id| !seen.contains(id))
        .collect();
    assert!(
        missing.is_empty(),
        "following the published recipe exactly — including its own stop rule — \
         collected {} of {corpus} documents over {pages} pages, silently missing {} \
         (first few: {:?}). This is the truncated-export failure the scroll 400 above \
         it exists to prevent, reintroduced in the remediation for it.",
        seen.len(),
        missing.len(),
        &missing[..missing.len().min(5)]
    );
    assert_eq!(
        pages, claimed_pages,
        "the transcript claims the walk takes {claimed_pages} pages; it took {pages}"
    );
}
