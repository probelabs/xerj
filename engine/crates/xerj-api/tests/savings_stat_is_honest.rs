//! The `_savings` statistic, from the outside.
//!
//! XERJ's pitch is that it returns answers rather than payloads. `_savings`
//! puts a number on that — which makes it the one number in the project that
//! absolutely must not flatter itself. A metric caught inflating would take
//! every other number XERJ publishes down with it.
//!
//! So the tests here are weighted towards *absence*: the cases where the
//! honest output is nothing at all outnumber the cases where a block appears.
//!
//! **The baseline is the load-bearing part.** A blind UX dogfood found the
//! first revision measuring against a response containing raw embedding
//! vectors — a counterfactual no caller would ever request, and one this
//! engine has not produced by default since #309. It overstated realised
//! savings by a consistent ~9.3x and fired on queries where the caller had
//! projected nothing at all. The oracle used here is therefore a **second
//! real request with no `_source` clause** — literally the response the
//! caller would otherwise have received — and the assertion is that the
//! claimed saving equals the difference between the two.
//!
//! The statistic is ON by default. `?savings=exact` asks for a byte-for-byte
//! count; the default `sampled` mode extrapolates long arrays and says so in
//! the payload. Exactness assertions therefore run against `?savings=exact`,
//! and the default mode is held to a stated tolerance against it.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;
use xerj_common::types::{FieldConfig, FieldType, Schema};

/// An index whose `body` field carries a generated embedding companion — the
/// mechanism that saves the most bytes in practice.
async fn app_with_companions() -> (axum::Router, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = xerj_common::config::Config::default();
    config.server.data_dir = dir.path().to_string_lossy().into_owned();
    config.storage.wal_sync = xerj_common::config::WalSync::Async;
    let metrics = xerj_common::metrics::Metrics::new().expect("metrics");
    let engine = xerj_engine::Engine::new(config.clone()).expect("engine");
    let state = xerj_api::state::AppState::new(config, engine, metrics);

    let mut schema = Schema::empty();
    let mut body = FieldConfig::new("body", FieldType::Text);
    body.options.dimensions = Some(64);
    body.options.similarity = Some("cosine".into());
    body.embedding = Some(xerj_common::types::EmbeddingConfig {
        endpoint: None,
        model: None,
        target_field: Some("body_vector".into()),
    });
    schema.add_field(body).expect("body field");
    let mut companion = FieldConfig::new("body_vector", FieldType::Vector);
    companion.options.dimensions = Some(64);
    companion.options.similarity = Some("cosine".into());
    schema.add_field(companion).expect("body_vector field");
    schema
        .add_field(FieldConfig::new("title", FieldType::Text))
        .expect("title field");
    schema
        .add_field(FieldConfig::new("author", FieldType::Keyword))
        .expect("author field");

    state.engine.create_index("docs", schema).expect("create");
    let idx = state.engine.get_index("docs").expect("get");
    for n in 0..3 {
        idx.index_document(
            Some(n.to_string()),
            json!({
                "title": format!("quarterly report {n}"),
                "author": "finance",
                "body": "revenue recognition and deferred costs. ".repeat(40),
            }),
        )
        .await
        .expect("index document");
    }
    idx.refresh().await.expect("refresh");

    (xerj_api::router::build_es_compat_router(state), dir)
}

async fn search(app: &axum::Router, uri: &str, body: Value) -> Value {
    let response = app
        .clone()
        .oneshot(
            Request::post(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK, "search {uri} failed");
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    serde_json::from_slice(&bytes).expect("json body")
}

fn hits(response: &Value) -> &Vec<Value> {
    response["hits"]["hits"].as_array().expect("hits array")
}

/// Sum the wire size of every returned `_source`, computed here with plain
/// `serde_json` so nothing in the implementation is taken on trust.
fn source_bytes(response: &Value) -> u64 {
    hits(response)
        .iter()
        .map(|h| {
            serde_json::to_vec(&h["_source"])
                .expect("source json")
                .len() as u64
        })
        .sum()
}

// ─────────────────────────────────────────────────────────────────────────────
// A saving happened: the number must be right, and checkable
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_projection_is_measured_against_the_response_you_would_have_got() {
    let (app, _dir) = app_with_companions().await;

    let projected = search(
        &app,
        "/docs/_search?savings=exact",
        json!({ "query": { "match_all": {} }, "size": 3, "_source": ["title"] }),
    )
    .await;
    let savings = &projected["_savings"];
    assert!(
        savings.is_object(),
        "dropping the body from three documents is a real saving: {projected}"
    );

    // The oracle: the same query with NO `_source` clause — the response this
    // caller would actually have received had they not projected. A second
    // round trip through the real server, not a re-run of any arithmetic.
    let default_shape = search(
        &app,
        "/docs/_search?savings=false",
        json!({ "query": { "match_all": {} }, "size": 3 }),
    )
    .await;
    let observed = source_bytes(&default_shape) - source_bytes(&projected);

    assert_eq!(
        savings["bytes"].as_u64().expect("bytes"),
        observed,
        "the claim must equal what this caller's own projection actually shed"
    );

    // And prove the old baseline would have been very different, so the
    // assertion above is not vacuous.
    let everything = search(
        &app,
        "/docs/_search?savings=false",
        json!({ "query": { "match_all": {} }, "size": 3, "_source": true }),
    )
    .await;
    let against_stored = source_bytes(&everything) - source_bytes(&projected);
    assert!(
        against_stored > observed * 2,
        "this corpus must have companions worth counting ({against_stored} vs {observed}), \
         or the regression this test guards could not recur"
    );
}

#[tokio::test]
async fn the_engines_own_default_is_never_claimed_as_a_saving() {
    // The reported failure, verbatim: `size: 20`, full source, no projection
    // of any kind — and a block claiming 22,298,691 bytes saved on a 2.7 MB
    // response, because the engine was counting its own default behaviour.
    let (app, _dir) = app_with_companions().await;
    let response = search(
        &app,
        "/docs/_search",
        json!({ "query": { "match_all": {} }, "size": 3 }),
    )
    .await;
    assert!(
        !hits(&response).is_empty(),
        "the query itself must return documents: {response}"
    );
    assert!(
        response.get("_savings").is_none(),
        "the caller made no effort and asked for everything; there is nothing to \
         congratulate them for: {response}"
    );
}

#[tokio::test]
async fn the_stat_is_on_by_default_and_labelled_as_an_estimate() {
    let (app, _dir) = app_with_companions().await;
    let query = json!({ "query": { "match_all": {} }, "size": 3, "_source": ["title"] });

    let default_mode = search(&app, "/docs/_search", query.clone()).await;
    let savings = &default_mode["_savings"];
    assert!(
        savings.is_object(),
        "the statistic rides on a stock search, with no opt-in: {default_mode}"
    );
    assert_eq!(
        savings["measured"].as_str().expect("measured"),
        "sampled",
        "the default figure is an extrapolation and must be labelled as one"
    );

    let exact_mode = search(&app, "/docs/_search?savings=exact", query).await;
    let sampled = savings["bytes"].as_u64().expect("sampled bytes") as f64;
    let exact = exact_mode["_savings"]["bytes"].as_u64().expect("exact") as f64;
    let error = (sampled - exact) / exact * 100.0;
    assert!(
        error.abs() <= 1.0,
        "sampled {sampled} vs exact {exact} is {error:+.2}% — an estimate this far out \
         is not worth publishing"
    );
    assert!(
        exact_mode["_savings"].get("measured").is_none(),
        "an exact figure carries no hedge; absence is the label"
    );
}

#[tokio::test]
async fn the_block_carries_only_the_measured_number_and_a_sentence() {
    let (app, _dir) = app_with_companions().await;
    let response = search(
        &app,
        "/docs/_search",
        json!({ "query": { "match_all": {} }, "size": 3, "_source": ["title"] }),
    )
    .await;
    let block = response["_savings"].as_object().expect("a block");
    let keys: Vec<&str> = block.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        vec!["bytes", "measured", "note"],
        "44% of the previous block was two constants and a division by four"
    );
}

#[tokio::test]
async fn a_two_field_projection_reads_differently_from_withholding_source() {
    let (app, _dir) = app_with_companions().await;

    let projection = search(
        &app,
        "/docs/_search?savings=exact",
        json!({ "query": { "match_all": {} }, "size": 3, "_source": ["title"] }),
    )
    .await;
    let withheld = search(
        &app,
        "/docs/_search?savings=exact",
        json!({ "query": { "match_all": {} }, "size": 3, "_source": false }),
    )
    .await;

    let projection_note = projection["_savings"]["note"].as_str().expect("note");
    let withheld_note = withheld["_savings"]["note"].as_str().expect("note");
    assert_ne!(
        projection_note, withheld_note,
        "two different mechanisms fired; one sentence for both teaches nobody anything"
    );
    assert_eq!(
        projection_note,
        "_source narrowed to the 1 field you listed"
    );
    assert_eq!(withheld_note, "_source withheld, as requested");
}

// ─────────────────────────────────────────────────────────────────────────────
// The reported bugs
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn the_note_counts_what_you_asked_for_not_what_came_back() {
    // Reported: asking for one field the document lacks produced
    // "Sent the 0 fields you asked for" — the caller's own request, reported
    // back to them wrongly.
    let (app, _dir) = app_with_companions().await;
    let response = search(
        &app,
        "/docs/_search",
        json!({ "query": { "match_all": {} }, "size": 3, "_source": ["no_such_field"] }),
    )
    .await;
    assert_eq!(
        response["_savings"]["note"].as_str().expect("note"),
        "_source narrowed to the 1 field you listed",
        "one field was listed, whether or not any document had it: {response}"
    );
}

#[tokio::test]
async fn the_note_does_not_move_with_page_size() {
    // Reported: the `not all M` denominator went 8 -> 11 -> 12 -> 15 on the
    // same query at different `size` values, because it described the widest
    // document on the page rather than the schema. There is no denominator
    // now, so a heterogeneous corpus cannot move the sentence.
    let (app, _dir) = app_with_companions().await;
    let mut notes = Vec::new();
    for size in [1, 2, 3] {
        let response = search(
            &app,
            "/docs/_search",
            json!({ "query": { "match_all": {} }, "size": size, "_source": ["title"] }),
        )
        .await;
        notes.push(
            response["_savings"]["note"]
                .as_str()
                .unwrap_or("(absent)")
                .to_string(),
        );
    }
    assert_eq!(
        notes[0], notes[2],
        "the same query at two page sizes must not report two different shapes"
    );
    assert_eq!(notes[1], notes[2]);
}

#[tokio::test]
async fn the_better_targeted_query_is_instrumented_and_netted() {
    // Reported: `_source: false` alone produced a block, but adding a
    // `fields` projection on top — the strictly better query — made the block
    // vanish, because that path declined to claim anything. It now claims,
    // net of the bytes `fields` actually put back on the wire.
    let (app, _dir) = app_with_companions().await;
    let base = json!({ "query": { "match_all": {} }, "size": 3, "_source": false });
    let mut with_fields = base.as_object().cloned().unwrap();
    with_fields.insert("fields".to_string(), json!(["title"]));

    let plain = search(&app, "/docs/_search?savings=exact", base).await;
    let targeted = search(
        &app,
        "/docs/_search?savings=exact",
        Value::Object(with_fields),
    )
    .await;

    let targeted_savings = &targeted["_savings"];
    assert!(
        targeted_savings.is_object(),
        "the better-targeted query must not lose its block: {targeted}"
    );
    assert_eq!(
        targeted_savings["note"].as_str().expect("note"),
        "fields returned in place of _source"
    );

    let plain_bytes = plain["_savings"]["bytes"].as_u64().expect("plain bytes");
    let targeted_bytes = targeted_savings["bytes"].as_u64().expect("targeted bytes");
    assert!(
        targeted_bytes < plain_bytes,
        "the `fields` values ARE on the wire ({targeted_bytes} vs {plain_bytes}); \
         a claim that ignored them would be gross, not net"
    );

    // Independent check of the netting: the difference between the two claims
    // is the size of the `fields` objects the second response actually carries.
    let emitted: u64 = hits(&targeted)
        .iter()
        .filter_map(|h| h.get("fields"))
        .map(|f| serde_json::to_vec(f).expect("fields json").len() as u64)
        .sum();
    assert_eq!(
        plain_bytes - targeted_bytes,
        emitted,
        "exactly the re-materialised bytes, no more and no less"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Nothing happened, or too little happened: the correct output is nothing.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_saving_too_small_to_be_worth_printing_is_not_printed() {
    // Reported: 135 bytes of block to announce a 19-byte saving, net -116,
    // on ordinary single-document lookups. This fixture reproduces that
    // number exactly — projecting 2 of the 3 user fields drops only the
    // short `author` keyword.
    let (app, _dir) = app_with_companions().await;
    let query = json!({
        "query": { "match_all": {} },
        "size": 1,
        "_source": ["title", "body"],
    });
    let response = search(&app, "/docs/_search", query).await;
    assert!(
        response.get("_savings").is_none(),
        "a 19-byte saving cannot justify a hundred-byte block: {response}"
    );
}

#[tokio::test]
async fn an_explicit_filter_that_re_admits_the_vectors_claims_nothing() {
    // `_source: {excludes: [...]}` overrides the default projection wholesale,
    // so the generated companions come BACK — the response is larger than the
    // one the caller would have received by writing nothing at all. That is
    // the opposite of a saving, and `saturating_sub` must not let the
    // shortfall be reported as a win.
    let (app, _dir) = app_with_companions().await;
    let response = search(
        &app,
        "/docs/_search",
        json!({
            "query": { "match_all": {} },
            "size": 1,
            "_source": { "excludes": ["author"] },
        }),
    )
    .await;
    let default_shape = search(
        &app,
        "/docs/_search?savings=false",
        json!({ "query": { "match_all": {} }, "size": 1 }),
    )
    .await;
    assert!(
        source_bytes(&response) > source_bytes(&default_shape),
        "this fixture must actually return more than the default, or it proves nothing"
    );
    assert!(
        response.get("_savings").is_none(),
        "a response bigger than the default withheld nothing: {response}"
    );
}

#[tokio::test]
async fn asking_for_the_whole_document_reports_nothing() {
    let (app, _dir) = app_with_companions().await;
    let response = search(
        &app,
        "/docs/_search",
        json!({ "query": { "match_all": {} }, "size": 3, "_source": true }),
    )
    .await;
    assert!(
        response.get("_savings").is_none(),
        "`_source: true` ships more than the default, not less: {response}"
    );
}

#[tokio::test]
async fn a_projection_that_keeps_every_field_reports_nothing() {
    let (app, _dir) = app_with_companions().await;
    let response = search(
        &app,
        "/docs/_search",
        json!({
            "query": { "match_all": {} },
            "size": 3,
            "_source": ["title", "author", "body"],
        }),
    )
    .await;
    assert!(
        response.get("_savings").is_none(),
        "asking for all of the fields withholds none of them: {response}"
    );
}

#[tokio::test]
async fn an_aggregation_only_query_reports_nothing() {
    let (app, _dir) = app_with_companions().await;
    let response = search(
        &app,
        "/docs/_search",
        json!({
            "query": { "match_all": {} },
            "size": 0,
            "aggs": { "by_author": { "terms": { "field": "author" } } },
        }),
    )
    .await;
    assert!(
        response["aggregations"]["by_author"]["buckets"]
            .as_array()
            .is_some_and(|b| !b.is_empty()),
        "the aggregation itself must still work: {response}"
    );
    assert!(
        response.get("_savings").is_none(),
        "the caller asked for zero documents and received zero documents. Claiming the \
         documents they never requested as a 'saving' is exactly the hypothetical this \
         metric refuses to invent: {response}"
    );
}

#[tokio::test]
async fn a_query_that_matches_nothing_reports_nothing() {
    let (app, _dir) = app_with_companions().await;
    let response = search(
        &app,
        "/docs/_search",
        json!({ "query": { "term": { "author": "nobody" } }, "_source": ["title"] }),
    )
    .await;
    assert!(hits(&response).is_empty(), "expected no hits: {response}");
    assert!(
        response.get("_savings").is_none(),
        "no documents means no document bytes were withheld: {response}"
    );
}

#[tokio::test]
async fn opting_out_removes_the_key_entirely() {
    let (app, _dir) = app_with_companions().await;
    let query = json!({ "query": { "match_all": {} }, "size": 3, "_source": ["title"] });

    // Warm the query cache with a measured run first. `savings` is an
    // internal request field, so it has to be mixed into the cache key
    // explicitly — without that, this opted-out request is served the
    // previous one's record and a stock ES response grows a foreign key.
    // Found on a live server; this is its regression test.
    let _ = search(&app, "/docs/_search?savings=exact", query.clone()).await;

    let response = search(&app, "/docs/_search?savings=false", query).await;
    assert!(
        response.get("_savings").is_none(),
        "`?savings=false` must remove the key, cache hit or not: {response}"
    );
}
