//! Issue #204, from the outside: configuration xerj cannot honour must be
//! refused at the door, or refused at the point of use — never accepted with a
//! 200 and then quietly not done.
//!
//! Every case here was found by measurement against the first cut of this
//! branch, and each one is the HTTP-level statement of a defect that had only
//! engine-level (or no) coverage:
//!
//! 1. the create-time analysis gate had no HTTP test at all — neither its
//!    status code nor its ES error shape;
//! 2. the gate judged a declared filter by its `type` while `apply_settings`
//!    resolves it by NAME, so the canonical Elasticsearch-docs
//!    `english_stop` block started 400ing although xerj serves it correctly;
//! 3. `PUT /{index}/_settings` walked straight around the gate — it merged an
//!    `analysis` block into the display copy and rebuilt nothing;
//! 4. `PUT /_ingest/pipeline` was redefined by this branch to mean "compiled ⇒
//!    xerj honours this", and then accepted an ES `grok` `patterns` array, a
//!    processor-level `if` guard, and an `append` it executed as `set`;
//! 5. `_clone` copied at most 10,000 documents and swallowed every write
//!    error, then answered `{"acknowledged": true}`.
//!
//! Elasticsearch is referenced for semantics only. It is AGPL-3.0/SSPL-1.0/
//! Elastic-2.0 licensed and no code from it is reproduced here.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

fn app() -> (axum::Router, tempfile::TempDir) {
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
    let res = app.clone().oneshot(req).await.expect("response");
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 4 * 1024 * 1024)
        .await
        .expect("body");
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

fn put(path: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("request")
}

fn post(path: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("request")
}

fn get(path: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(path)
        .body(Body::empty())
        .expect("request")
}

// ── The create-time analysis gate ────────────────────────────────────────────

#[tokio::test]
async fn an_analysis_block_we_cannot_honour_is_a_400_in_the_es_error_shape() {
    let (app, _dir) = app();
    let (status, body) = send(
        &app,
        put(
            "/autocomplete",
            json!({
                "settings": { "analysis": { "analyzer": {
                    "ac": { "type": "custom", "tokenizer": "edge_ngram_tok" }
                } } }
            }),
        ),
    )
    .await;

    // Pre-fix this was `200 {"acknowledged": true}` and the index tokenised
    // with `standard` — an autocomplete index that matches nothing.
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["status"], 400, "{body}");
    assert!(body["error"]["type"].is_string(), "{body}");
    assert!(body["error"]["root_cause"].is_array(), "{body}");
    assert!(
        body["error"]["reason"]
            .as_str()
            .expect("reason")
            .contains("edge_ngram_tok"),
        "the reason must name the construct we cannot honour: {body}"
    );

    // And nothing was left behind.
    let (status, _) = send(&app, get("/autocomplete")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_canonical_elasticsearch_analysis_block_is_still_accepted() {
    let (app, _dir) = app();
    // Straight from the Elasticsearch docs' `rebuilt_english`, cut to the parts
    // xerj implements. `english_stop` declares `type: stop`, which
    // `apply_settings` cannot BUILD — and then resolves by NAME to the built-in
    // English stopword filter, which is exactly what it declares. Judging it on
    // `type` alone made this a 400: a straight regression on a settings block
    // xerj had always accepted and analysed correctly.
    let (status, body) = send(
        &app,
        put(
            "/blog",
            json!({
                "settings": { "analysis": {
                    "filter": { "english_stop": { "type": "stop", "stopwords": "_english_" } },
                    "analyzer": { "rebuilt_english": {
                        "type": "custom",
                        "tokenizer": "standard",
                        "filter": ["lowercase", "english_stop"]
                    } }
                } },
                "mappings": { "properties": {
                    "body": { "type": "text", "analyzer": "rebuilt_english" }
                } }
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["acknowledged"], true, "{body}");
}

#[tokio::test]
async fn analysis_cannot_be_smuggled_past_the_gate_through_put_settings() {
    let (app, _dir) = app();
    let (status, _) = send(&app, put("/blog", json!({}))).await;
    assert_eq!(status, StatusCode::OK);

    // The registry is built at create/open and there is no rebuild path, so
    // this could only ever have changed what `GET /_settings` echoes back.
    let (status, body) = send(
        &app,
        put(
            "/blog/_settings",
            json!({ "analysis": { "analyzer": {
                "late": { "type": "custom", "tokenizer": "whitespace" }
            } } }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"]["reason"]
            .as_str()
            .expect("reason")
            .contains("analysis"),
        "{body}"
    );

    // …and the display copy was not quietly updated either.
    let (_, settings) = send(&app, get("/blog/_settings")).await;
    assert!(
        settings.pointer("/blog/settings/index/analysis").is_none(),
        "a refused settings PUT must not leave the analysis block behind: {settings}"
    );

    // A settings PUT that xerj *can* honour is unaffected.
    let (status, _) = send(
        &app,
        put(
            "/blog/_settings",
            json!({ "index": { "number_of_replicas": 0 } }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

// ── PUT /_ingest/pipeline: compiled ⇒ honoured ───────────────────────────────

/// Register `pipeline`, then write one document through it. Returns the write's
/// status and body.
async fn write_through(
    app: &axum::Router,
    id: &str,
    processors: Value,
    doc: Value,
) -> (StatusCode, Value) {
    let (status, body) = send(
        app,
        put(
            &format!("/_ingest/pipeline/{id}"),
            json!({ "processors": processors }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "pipeline definition: {body}");
    assert_eq!(body["acknowledged"], true, "{body}");
    send(
        app,
        put(&format!("/docs/_doc/1?pipeline={id}&refresh=true"), doc),
    )
    .await
}

#[tokio::test]
async fn an_es_grok_processor_is_accepted_and_then_refuses_the_write() {
    let (app, _dir) = app();
    // Measured pre-fix: this compiled to the SYSLOG default, answered 200, and
    // indexed `{"message": "10.0.0.1 GET"}` with no `client` field anywhere.
    let (status, body) = write_through(
        &app,
        "grok_pipe",
        json!([{ "grok": { "field": "message", "patterns": ["%{IP:client} %{WORD:method}"] } }]),
        json!({ "message": "10.0.0.1 GET" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let reason = body["error"]["reason"].as_str().expect("reason");
    assert!(reason.contains("patterns"), "{body}");

    // The document was NOT indexed untransformed.
    let (status, _) = send(&app, get("/docs/_doc/1")).await;
    assert_ne!(status, StatusCode::OK, "the write must not have happened");
}

#[tokio::test]
async fn a_processor_level_if_guard_is_accepted_and_then_refuses_the_write() {
    let (app, _dir) = app();
    // Measured pre-fix: `{"foo": "something-else"}` was indexed as
    // `{"foo": "something-else", "env": "prod"}` — the guard excluded it.
    let (status, body) = write_through(
        &app,
        "guarded",
        json!([{ "set": { "field": "env", "value": "prod", "if": "ctx.foo == 'never'" } }]),
        json!({ "foo": "something-else" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"]["reason"]
            .as_str()
            .expect("reason")
            .contains("if"),
        "{body}"
    );
}

#[tokio::test]
async fn set_overrides_an_existing_value_like_elasticsearch() {
    let (app, _dir) = app();
    let (status, body) = write_through(
        &app,
        "setter",
        json!([{ "set": { "field": "env", "value": "prod" } }]),
        json!({ "env": "dev" }),
    )
    .await;
    assert!(status.is_success(), "{body}");

    let (_, doc) = send(&app, get("/docs/_doc/1")).await;
    // Pre-fix: `override` defaulted to false and this stayed "dev".
    assert_eq!(doc["_source"]["env"], "prod", "{doc}");
}

#[tokio::test]
async fn append_appends_rather_than_replacing() {
    let (app, _dir) = app();
    let (status, body) = write_through(
        &app,
        "appender",
        json!([{ "append": { "field": "tags", "value": "b" } }]),
        json!({ "tags": ["a"] }),
    )
    .await;
    assert!(status.is_success(), "{body}");

    let (_, doc) = send(&app, get("/docs/_doc/1")).await;
    // Pre-fix: `append` was mapped onto `set`, so this was `"b"` — while the
    // `_simulate` interpreter for the same pipeline produced `["a","b"]`.
    assert_eq!(doc["_source"]["tags"], json!(["a", "b"]), "{doc}");
}

#[tokio::test]
async fn convert_accepts_the_elasticsearch_spellings_of_the_same_conversion() {
    let (app, _dir) = app();
    // `long` is xerj's `integer` (i64) and `double` is its `float` (f64) — an
    // equivalent fallback, which issue #204 explicitly permits. Refusing them
    // 400ed previously-working ES pipelines for no gain.
    let (status, body) = write_through(
        &app,
        "converter",
        json!([
            { "convert": { "field": "count", "type": "long" } },
            { "convert": { "field": "ratio", "type": "double" } }
        ]),
        json!({ "count": "404", "ratio": "1.5" }),
    )
    .await;
    assert!(status.is_success(), "{body}");

    let (_, doc) = send(&app, get("/docs/_doc/1")).await;
    assert_eq!(doc["_source"]["count"], 404, "{doc}");
    assert_eq!(doc["_source"]["ratio"], 1.5, "{doc}");

    // `auto` has no equivalent here and stays refused.
    let (status, _) = send(
        &app,
        put(
            "/_ingest/pipeline/auto_conv",
            json!({ "processors": [{ "convert": { "field": "c", "type": "auto" } }] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ── _clone copies everything, or says it did not ─────────────────────────────

/// More documents than one clone page (1,000) and more than the old hardcoded
/// `size: 10000` ceiling, so the test fails on both the truncation and the
/// paging bug rather than only the second.
#[tokio::test]
async fn clone_copies_every_document_not_the_first_ten_thousand() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = xerj_common::config::Config::default();
    config.server.data_dir = dir.path().to_string_lossy().into_owned();
    config.storage.wal_sync = xerj_common::config::WalSync::Async;
    let metrics = xerj_common::metrics::Metrics::new().expect("metrics");
    let engine = xerj_engine::Engine::new(config.clone()).expect("engine");
    let state = xerj_api::state::AppState::new(config, engine, metrics);

    const DOCS: usize = 10_001;
    state
        .engine
        .create_index("src", xerj_common::types::Schema::empty())
        .expect("create_index");
    let idx = state.engine.get_index("src").expect("get_index");
    for i in 0..DOCS {
        idx.index_document(Some(format!("{i:08}")), json!({ "n": i }))
            .await
            .expect("index_document");
    }
    idx.refresh().await.expect("refresh");

    let app = xerj_api::router::build_es_compat_router(state);

    // The source really does hold more than the old ceiling, and the single
    // `size: 10000` search the pre-fix clone used really does stop at it — so
    // the assertion below fails on the old code rather than passing vacuously.
    let (_, src_count) = send(&app, get("/src/_count")).await;
    assert_eq!(
        src_count["count"].as_u64(),
        Some(DOCS as u64),
        "{src_count}"
    );
    let (_, capped) = send(
        &app,
        post(
            "/src/_search",
            json!({ "query": { "match_all": {} }, "size": 10000 }),
        ),
    )
    .await;
    assert_eq!(
        capped["hits"]["hits"].as_array().map(Vec::len),
        Some(10_000),
        "the pre-fix clone's one-shot search tops out here"
    );

    let (status, body) = send(&app, post("/src/_clone/dst", json!({}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["acknowledged"], true, "{body}");

    let (_, count) = send(&app, get("/dst/_count")).await;
    // Pre-fix: exactly 10000 — a partial copy reported as a completed one.
    assert_eq!(
        count["count"].as_u64(),
        Some(DOCS as u64),
        "clone must copy every document: {count}"
    );
}
