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
    let app = app_in(dir.path());
    (app, dir)
}

/// A node on an EXISTING data dir — i.e. a restart. Dropping the previous
/// router and building another one over the same directory replays
/// `cluster_state.json` exactly as a process restart does; it is the only way
/// to reach the boot path from a request-level test.
fn app_in(data_dir: &std::path::Path) -> axum::Router {
    let mut config = xerj_common::config::Config::default();
    config.server.data_dir = data_dir.to_string_lossy().into_owned();
    config.storage.wal_sync = xerj_common::config::WalSync::Async;
    let metrics = xerj_common::metrics::Metrics::new().expect("metrics");
    let engine = xerj_engine::Engine::new(config.clone()).expect("engine");
    let state = xerj_api::state::AppState::new(config, engine, metrics);
    xerj_api::router::build_es_compat_router(state)
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

// ── Every ingest path, not just the single-document one ──────────────────────
//
// The first cut of this branch made `PUT /_ingest/pipeline` mean "compiled ⇒
// xerj honours this" and wired the refusal into the three single-document
// handlers only. Measured against that build, `_bulk?pipeline=`, an index
// carrying `index.default_pipeline`, `_reindex` `dest.pipeline` and
// `_update_by_query?pipeline=` all answered 200/201 and stored the document
// UNTRANSFORMED — including for a pipeline that was perfectly runnable.

fn ndjson(path: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/x-ndjson")
        .body(Body::from(body.to_string()))
        .expect("request")
}

/// `{"set": {"field": "env", "value": "prod"}}` — compiles and runs.
async fn define_runnable_pipeline(app: &axum::Router, id: &str) {
    let (status, body) = send(
        app,
        put(
            &format!("/_ingest/pipeline/{id}"),
            json!({ "processors": [{ "set": { "field": "env", "value": "prod" } }] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// An ES `grok` `patterns` array — accepted by `PUT`, recorded unrunnable.
async fn define_unrunnable_pipeline(app: &axum::Router, id: &str) {
    let (status, body) = send(
        app,
        put(
            &format!("/_ingest/pipeline/{id}"),
            json!({ "processors": [{ "grok": {
                "field": "message",
                "patterns": ["%{IP:client} %{WORD:method}"]
            } }] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["acknowledged"], true, "{body}");
}

#[tokio::test]
async fn bulk_runs_the_url_level_pipeline() {
    let (app, _dir) = app();
    define_runnable_pipeline(&app, "setp").await;

    let (status, body) = send(
        &app,
        ndjson(
            "/_bulk?pipeline=setp&refresh=true",
            "{\"index\":{\"_index\":\"b\",\"_id\":\"1\"}}\n{\"a\":1}\n",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["errors"], false, "{body}");

    let (_, doc) = send(&app, get("/b/_doc/1")).await;
    // Pre-fix: `{"a": 1}` — the parameter was read by nothing.
    assert_eq!(doc["_source"]["env"], "prod", "{doc}");
}

#[tokio::test]
async fn bulk_refuses_an_unrunnable_url_level_pipeline() {
    let (app, _dir) = app();
    define_unrunnable_pipeline(&app, "unrun").await;

    let (status, body) = send(
        &app,
        ndjson(
            "/_bulk?pipeline=unrun",
            "{\"index\":{\"_index\":\"b\",\"_id\":\"1\"}}\n{\"message\":\"10.0.0.1 GET\"}\n",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["errors"], true, "{body}");
    assert_eq!(body["items"][0]["index"]["status"], 400, "{body}");
    // A failed item carries no `result`. Pre-fix it said `"deleted"`, which is
    // a statement about the document that was not true.
    assert!(
        body["items"][0]["index"].get("result").is_none(),
        "a failed item must not claim a result: {body}"
    );

    let (status, _) = send(&app, get("/b/_doc/1")).await;
    assert_ne!(status, StatusCode::OK, "nothing may have been written");
}

#[tokio::test]
async fn bulk_honours_index_default_pipeline() {
    let (app, _dir) = app();
    define_runnable_pipeline(&app, "setp").await;
    let (status, body) = send(
        &app,
        put(
            "/dp",
            json!({ "settings": { "index": { "default_pipeline": "setp" } } }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (_, body) = send(
        &app,
        ndjson(
            "/_bulk?refresh=true",
            "{\"index\":{\"_index\":\"dp\",\"_id\":\"1\"}}\n{\"a\":1}\n",
        ),
    )
    .await;
    assert_eq!(body["errors"], false, "{body}");
    let (_, doc) = send(&app, get("/dp/_doc/1")).await;
    // Pre-fix: `_bulk` never consulted the setting, so this was `{"a": 1}`
    // while `PUT /dp/_doc/1` on the same index ran the pipeline.
    assert_eq!(doc["_source"]["env"], "prod", "{doc}");

    // `_none` still disables it, as in Elasticsearch.
    let (_, body) = send(
        &app,
        ndjson(
            "/_bulk?pipeline=_none&refresh=true",
            "{\"index\":{\"_index\":\"dp\",\"_id\":\"2\"}}\n{\"a\":2}\n",
        ),
    )
    .await;
    assert_eq!(body["errors"], false, "{body}");
    let (_, doc) = send(&app, get("/dp/_doc/2")).await;
    assert!(doc["_source"].get("env").is_none(), "{doc}");
}

#[tokio::test]
async fn reindex_runs_and_validates_dest_pipeline() {
    let (app, _dir) = app();
    define_runnable_pipeline(&app, "setp").await;
    define_unrunnable_pipeline(&app, "unrun").await;
    let (status, _) = send(&app, put("/rsrc/_doc/1?refresh=true", json!({ "a": 1 }))).await;
    assert!(status.is_success());

    // Unrunnable: refused before anything is copied.
    let (status, body) = send(
        &app,
        post(
            "/_reindex",
            json!({ "source": { "index": "rsrc" },
                    "dest": { "index": "rbad", "pipeline": "unrun" } }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, _) = send(&app, get("/rbad/_count")).await;
    assert_ne!(status, StatusCode::OK, "no destination may have been made");

    // Runnable: every copied document really went through it. Pre-fix
    // `dest.pipeline` was not even a field on the request struct, so serde
    // dropped it and the corpus was copied verbatim under `"created": 1`.
    let (status, body) = send(
        &app,
        post(
            "/_reindex",
            json!({ "source": { "index": "rsrc" },
                    "dest": { "index": "rdst", "pipeline": "setp" } }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["created"], 1, "{body}");
    let (_, doc) = send(&app, get("/rdst/_doc/1")).await;
    assert_eq!(doc["_source"]["env"], "prod", "{doc}");
}

#[tokio::test]
async fn update_by_query_runs_and_validates_its_pipeline() {
    let (app, _dir) = app();
    define_runnable_pipeline(&app, "setp").await;
    define_unrunnable_pipeline(&app, "unrun").await;
    let (status, _) = send(&app, put("/ubq/_doc/1?refresh=true", json!({ "a": 1 }))).await;
    assert!(status.is_success());

    let (status, body) = send(
        &app,
        post(
            "/ubq/_update_by_query?pipeline=unrun",
            json!({ "query": { "match_all": {} } }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (_, doc) = send(&app, get("/ubq/_doc/1")).await;
    assert_eq!(doc["_version"], 1, "nothing may have been rewritten: {doc}");

    let (status, body) = send(
        &app,
        post(
            "/ubq/_update_by_query?pipeline=setp&refresh=true",
            json!({ "query": { "match_all": {} } }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["updated"], 1, "{body}");
    let (_, doc) = send(&app, get("/ubq/_doc/1")).await;
    // Pre-fix: `"updated": 1` and the document byte-identical to what it was.
    assert_eq!(doc["_source"]["env"], "prod", "{doc}");
}

#[tokio::test]
async fn a_pipeline_level_on_failure_block_is_accepted_and_then_refuses_the_write() {
    let (app, _dir) = app();
    // The same key one level up from the processor-level `on_failure` the
    // first cut refused. Measured against that build: `PUT` answered 200, `GET`
    // echoed the block back, and the write succeeded with the recovery chain
    // silently absent.
    let (status, body) = send(
        &app,
        put(
            "/_ingest/pipeline/plof",
            json!({
                "processors": [{ "set": { "field": "env", "value": "prod" } }],
                "on_failure": [{ "set": { "field": "err", "value": "yes" } }],
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["acknowledged"], true, "{body}");

    let (status, body) = send(&app, put("/plof/_doc/1?pipeline=plof", json!({ "a": 1 }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("on_failure")),
        "{body}"
    );
}

#[tokio::test]
async fn set_copy_from_is_a_capability_gap_not_a_caller_error() {
    let (app, _dir) = app();
    // `copy_from` is valid Elasticsearch 7.x+. Refusing the DEFINITION with
    // `mapper_parsing_exception: missing value` told the caller their correct
    // pipeline was malformed; the gap is xerj's, so it takes the same
    // accept-then-refuse-the-write route as every other unsupported option.
    let (status, body) = send(
        &app,
        put(
            "/_ingest/pipeline/cf",
            json!({ "processors": [{ "set": { "field": "b", "copy_from": "a" } }] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["acknowledged"], true, "{body}");

    let (status, body) = send(&app, put("/cf/_doc/1?pipeline=cf", json!({ "a": 1 }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("copy_from")),
        "{body}"
    );
}

#[tokio::test]
async fn all_stats_reports_the_schema_persist_failure_counter_too() {
    let (app, _dir) = app();
    let (status, _) = send(&app, put("/s1/_doc/1?refresh=true", json!({ "a": 1 }))).await;
    assert!(status.is_success());

    let (status, body) = send(&app, get("/_all/_stats")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // Pre-fix `_all.primaries` carried no `mappings` key at all, while
    // `Index::schema_persist_failures`'s doc-comment named this endpoint.
    assert_eq!(
        body["_all"]["primaries"]["mappings"]["schema_persist_failures"], 0,
        "{body}"
    );
    assert_eq!(
        body["indices"]["s1"]["primaries"]["mappings"]["schema_persist_failures"], 0,
        "{body}"
    );
}

#[tokio::test]
async fn a_synonym_filter_given_a_bare_string_is_refused_not_silently_emptied() {
    let (app, _dir) = app();
    // `apply_settings` reads `synonyms` with `and_then(as_array)`, so a bare
    // string built a synonym filter with ZERO rules — registered, referenced by
    // the analyzer, expanding nothing, reported nowhere.
    let (status, body) = send(
        &app,
        put(
            "/syn_bad",
            json!({ "settings": { "analysis": {
                "filter": { "s": { "type": "synonym", "synonyms": "fast,quick" } },
                "analyzer": { "a": { "type": "custom", "tokenizer": "standard",
                                     "filter": ["s"] } }
            } } }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("synonyms")),
        "{body}"
    );

    // The array form is still accepted.
    let (status, body) = send(
        &app,
        put(
            "/syn_ok",
            json!({ "settings": { "analysis": {
                "filter": { "s": { "type": "synonym", "synonyms": ["fast,quick"] } },
                "analyzer": { "a": { "type": "custom", "tokenizer": "standard",
                                     "filter": ["s"] } }
            } } }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

// ── Round 2 of the sweep: the gate's own blind spots ─────────────────────────

/// `_simulate` walks the STORED pipeline. Once ES `rename` started compiling,
/// the stored form became xerj's `stages` shape — and the stages→processors
/// conversion emitted the INTERNAL stage name, so `processor_type` read
/// `field_rename` where Elasticsearch reports `rename`. This is the ES-compat
/// YAML conformance case "Test verbose simulate with error in pipeline"
/// (`40_simulate.yml`) as a Rust test.
#[tokio::test]
async fn simulate_reports_elasticsearch_processor_names_for_a_compiled_pipeline() {
    let (app, _dir) = app();
    let (status, body) = send(
        &app,
        put(
            "/_ingest/pipeline/my_pipeline",
            json!({
                "description": "_description",
                "processors": [
                    { "rename": { "field": "does_not_exist", "target_field": "_value" } }
                ]
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["acknowledged"], true, "{body}");

    let (status, body) = send(
        &app,
        post(
            "/_ingest/pipeline/my_pipeline/_simulate?verbose=true",
            json!({ "docs": [{ "_index": "index", "_id": "id",
                               "_source": { "foo": "bar" } }] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let results = body
        .pointer("/docs/0/processor_results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("processor_results missing: {body}"));
    assert_eq!(results.len(), 1, "{body}");
    assert_eq!(
        results[0]["processor_type"], "rename",
        "the internal stage name must not leak into the simulate response: {body}"
    );
    assert_eq!(results[0]["status"], "error", "{body}");
    assert_eq!(
        results[0]["error"]["reason"], "field [does_not_exist] doesn't exist",
        "{body}"
    );

    // The `remove` translation round-trips too: `drop_field`'s `fields` array
    // must come back as ES's single `field`.
    let (status, body) = send(
        &app,
        put(
            "/_ingest/pipeline/rm",
            json!({ "processors": [{ "remove": { "field": "secret" } }] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, body) = send(
        &app,
        post(
            "/_ingest/pipeline/rm/_simulate?verbose=true",
            json!({ "docs": [{ "_source": { "secret": "s", "keep": 1 } }] }),
        ),
    )
    .await;
    let results = body
        .pointer("/docs/0/processor_results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("processor_results missing: {body}"));
    assert_eq!(results[0]["processor_type"], "remove", "{body}");
    assert_eq!(
        results[0].pointer("/doc/_source/secret"),
        None,
        "the simulated `remove` must actually remove: {body}"
    );
}

/// `ignore_failure: false` is Elasticsearch's OWN default and ES clients write
/// it out. Testing key presence refused it, so this pipeline registered and
/// then 400'd every write through it — while the byte-identical processor
/// without the key indexed fine.
#[tokio::test]
async fn spelling_out_the_elasticsearch_default_does_not_break_the_pipeline() {
    let (app, _dir) = app();
    let (status, body) = write_through(
        &app,
        "defaults_spelled_out",
        json!([{ "set": { "field": "env", "value": "prod",
                          "ignore_failure": false, "on_failure": [] } }]),
        json!({ "msg": "hello" }),
    )
    .await;
    assert!(
        status.is_success(),
        "ignore_failure:false is the ES default and must not make a pipeline \
         unrunnable: {body}"
    );
    let (_, doc) = send(&app, get("/docs/_doc/1")).await;
    assert_eq!(doc["_source"]["env"], "prod", "{doc}");

    // `ignore_failure: true` still asks for something this build cannot do.
    let (status, _) = send(
        &app,
        put(
            "/_ingest/pipeline/ignoring",
            json!({ "processors": [
                { "set": { "field": "e", "value": "p", "ignore_failure": true } }
            ] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "ES accepts it, so xerj stores it");
    let (status, body) = send(
        &app,
        put("/docs/_doc/9?pipeline=ignoring", json!({ "a": 1 })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

/// `remove` and `rename` built their xerj config from scratch, so a
/// processor-level `if` on them was DROPPED before the gate ever saw it: the
/// pipeline compiled, was acknowledged, and dropped the field on EVERY
/// document — the exact defect this sweep exists to eliminate, surviving
/// inside its own gate, and self-inconsistent with `set`, which refused the
/// identical key.
#[tokio::test]
async fn a_guard_on_remove_or_rename_is_not_dropped_by_the_translation() {
    for (name, processor) in [
        (
            "guarded_remove",
            json!({ "remove": { "field": "secret", "if": "ctx.tenant == 'a'" } }),
        ),
        (
            "guarded_rename",
            json!({ "rename": { "field": "a", "target_field": "b",
                                "if": "ctx.tenant == 'a'" } }),
        ),
        (
            "failing_remove",
            json!({ "remove": { "field": "secret", "ignore_failure": true } }),
        ),
        (
            "recovering_rename",
            json!({ "rename": { "field": "a", "target_field": "b",
                                "on_failure": [{ "set": { "field": "x", "value": 1 } }] } }),
        ),
    ] {
        let (app, _dir) = app();
        let (status, body) = write_through(
            &app,
            name,
            json!([processor]),
            json!({ "tenant": "b", "secret": "s", "a": 1 }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{name}: a processor-level key the gate refuses on `set` must not be \
             silently dropped on `remove`/`rename`: {body}"
        );

        // And the untransformed document was not indexed behind a 2xx.
        let (status, _) = send(&app, get("/docs/_doc/1")).await;
        assert_ne!(
            status,
            StatusCode::OK,
            "{name}: the write must not have happened"
        );
    }
}

/// The bare `remove`/`rename` still work — the translation was fixed by
/// editing the config rather than rebuilding it, so nothing was lost.
#[tokio::test]
async fn remove_and_rename_still_do_their_job() {
    let (app, _dir) = app();
    let (status, body) = write_through(
        &app,
        "cleanup",
        json!([
            { "remove": { "field": "secret" } },
            { "rename": { "field": "a", "target_field": "b" } }
        ]),
        json!({ "secret": "s", "a": 1 }),
    )
    .await;
    assert!(status.is_success(), "{body}");
    let (_, doc) = send(&app, get("/docs/_doc/1")).await;
    assert_eq!(doc.pointer("/_source/secret"), None, "{doc}");
    assert_eq!(doc["_source"]["b"], 1, "{doc}");
}

/// `ignore_missing: true` is exactly what every field-reading stage already
/// does, so it is honoured. `false` asks for the document to be FAILED on a
/// missing field, which no stage can do — `ProcessAction` has no error
/// variant — so it is refused rather than silently accepted.
#[tokio::test]
async fn ignore_missing_is_honoured_or_refused_never_silently_accepted() {
    let (app, _dir) = app();
    let (status, body) = write_through(
        &app,
        "lenient",
        json!([{ "rename": { "field": "nope", "target_field": "b",
                             "ignore_missing": true } }]),
        json!({ "a": 1 }),
    )
    .await;
    assert!(
        status.is_success(),
        "ignore_missing:true is honoured: {body}"
    );

    let (status, body) = send(
        &app,
        put(
            "/_ingest/pipeline/strict_missing",
            json!({ "processors": [
                { "rename": { "field": "nope", "target_field": "b",
                              "ignore_missing": false } }
            ] }),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "ES accepts it, so xerj stores it: {body}"
    );
    let (status, body) = send(
        &app,
        put("/docs/_doc/7?pipeline=strict_missing", json!({ "a": 1 })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "ignore_missing:false must not be silently accepted: {body}"
    );
    assert!(
        body["error"]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("ignore_missing")),
        "{body}"
    );
}

/// The `analysis` refusal in `PUT /{index}/_settings` tested two nested
/// pointers only. ES clients routinely send flat dotted keys, this handler
/// accepts them for every other setting, and `GET /_settings` then echoed back
/// an analyzer that analyses nothing.
#[tokio::test]
async fn the_dotted_analysis_spelling_does_not_walk_past_the_settings_gate() {
    for body in [
        json!({ "index.analysis.analyzer.x.type": "custom" }),
        json!({ "index": { "analysis.analyzer.x.type": "custom" } }),
        json!({ "analysis.analyzer.x.type": "custom" }),
        json!({ "index": { "analysis": { "analyzer": { "x": { "type": "custom" } } } } }),
    ] {
        let (app, _dir) = app();
        let (status, _) = send(&app, put("/blog", json!({}))).await;
        assert_eq!(status, StatusCode::OK);

        let (status, resp) = send(&app, put("/blog/_settings", body.clone())).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{body} must be refused like the nested spelling: {resp}"
        );

        // …and nothing was echoed back into the display copy.
        let (_, settings) = send(&app, get("/blog/_settings")).await;
        assert!(
            !settings.to_string().contains("analysis"),
            "a refused settings PUT must leave no analysis trace: {settings}"
        );
    }

    // A dotted setting that is NOT analysis is unaffected.
    let (app, _dir) = app();
    send(&app, put("/blog", json!({}))).await;
    let (status, body) = send(
        &app,
        put("/blog/_settings", json!({ "index.number_of_replicas": 0 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

// ─────────────────────────────────────────────────────────────────────────────
// A refusal must survive a restart.
//
// The boot replay softens config this build cannot honour, so that replacing
// the binary does not flip a working cluster's writes from 201 to 400. That
// door was opened for definitions written by a build with NO gate — but
// `CompileMode::ReplayPersisted` could not tell those from definitions THIS
// build had already refused, so a plain restart resurrected a refused pipeline
// and ran it with the refused option ignored. Verbatim repro below, using only
// xerj's own output.
// ─────────────────────────────────────────────────────────────────────────────

/// The exact sequence measured against the first cut of this branch:
/// PUT → GET → edit → PUT (refused, 400 on write) → restart → 201, and
/// `secret` dropped from a document the guard EXCLUDES.
#[tokio::test]
async fn a_refusal_recorded_at_put_time_survives_a_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = app_in(dir.path());

    // (1) a plain, runnable redaction pipeline.
    let (status, body) = send(
        &app,
        put(
            "/_ingest/pipeline/redact",
            json!({ "description": "redact",
                    "processors": [{ "remove": { "field": "secret" } }] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // (2) the operator reads it back and (3) adds a tenant guard to whatever
    // GET handed them, then PUTs it back. Both shapes must behave the same,
    // so drive the loop through the response body rather than a literal.
    let (_, stored) = send(&app, get("/_ingest/pipeline/redact")).await;
    let mut edited = stored["redact"].clone();
    let processors = edited["processors"]
        .as_array_mut()
        .expect("GET must answer in the Elasticsearch `processors` vocabulary");
    processors[0]["remove"]["if"] = json!("ctx.tenant == 'a'");
    let (status, body) = send(&app, put("/_ingest/pipeline/redact", edited)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "ES accepts a guard, so xerj stores it: {body}"
    );

    // (4) …and every write through it is refused, because the guard is not
    // evaluated.
    let (status, body) = send(
        &app,
        put(
            "/docs/_doc/1?pipeline=redact",
            json!({ "tenant": "b", "secret": "ssn-111" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    // (5) restart the node. Nothing else — no PUT, no operator action.
    drop(app);
    let app = app_in(dir.path());

    // (6) the identical write must still be refused.
    let (status, body) = send(
        &app,
        put(
            "/docs/_doc/2?pipeline=redact",
            json!({ "tenant": "b", "secret": "ssn-111" }),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a restart must not overturn a refusal the operator was already given: {body}"
    );
    assert!(
        body["error"]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("if")),
        "and it must answer with the same reason: {body}"
    );

    // (7) …and no document was written with `secret` dropped by a guard that
    // excludes it.
    let (status, doc) = send(&app, get("/docs/_doc/2")).await;
    assert_ne!(
        status,
        StatusCode::OK,
        "the guarded write must not have happened: {doc}"
    );
}

/// The same defect reached through the shape the engine itself persists.
///
/// `PUT /_ingest/pipeline/{id}` passes a body with no `processors` key straight
/// through as a xerj `stages` definition, and that body is what
/// `register_unrunnable_pipeline` stores. A `stages` body — unlike the raw
/// Elasticsearch one — deserialises perfectly at boot, so the replay recompiled
/// it softly and the refusal evaporated. This is the case the ES-vocabulary
/// GET does not incidentally cover, and the one the persisted marker is for.
#[tokio::test]
async fn a_refusal_survives_a_restart_for_a_stages_shaped_definition_too() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = app_in(dir.path());

    let stages = json!({
        "description": "redact",
        "stages": [{
            "type": "drop_field",
            "config": { "fields": ["secret"], "if": "ctx.tenant == 'a'" }
        }]
    });
    let (status, body) = send(&app, put("/_ingest/pipeline/redact_native", stages)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["acknowledged"], true, "{body}");

    let (status, body) = send(
        &app,
        put(
            "/docs/_doc/1?pipeline=redact_native",
            json!({ "tenant": "b", "secret": "ssn-111" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    // Restart. Nothing else.
    drop(app);
    let app = app_in(dir.path());

    let (status, body) = send(
        &app,
        put(
            "/docs/_doc/2?pipeline=redact_native",
            json!({ "tenant": "b", "secret": "ssn-111" }),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "measured before the persisted refusal marker: 201, and `secret` dropped from a \
         document the guard excludes: {body}"
    );
    let (status, doc) = send(&app, get("/docs/_doc/2")).await;
    assert_ne!(status, StatusCode::OK, "{doc}");
}

/// The marker is provenance, not a tombstone: a definition this build compiles
/// cleanly must not stay refused because some earlier build could not run it.
#[tokio::test]
async fn a_recorded_refusal_is_cleared_by_a_definition_that_compiles() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = app_in(dir.path());

    // Refused: `ignore_failure: true` is a decision this build cannot make.
    let (status, body) = send(
        &app,
        put(
            "/_ingest/pipeline/p",
            json!({ "processors": [
                { "set": { "field": "env", "value": "prod", "ignore_failure": true } }
            ] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _) = send(&app, put("/docs/_doc/1?pipeline=p", json!({ "a": 1 }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // The operator drops the option and re-PUTs.
    let (status, body) = send(
        &app,
        put(
            "/_ingest/pipeline/p",
            json!({ "processors": [{ "set": { "field": "env", "value": "prod" } }] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // It runs now, and still runs after a restart — the marker did not outlive
    // its cause.
    drop(app);
    let app = app_in(dir.path());
    let (status, body) = send(
        &app,
        put("/docs/_doc/2?pipeline=p&refresh=true", json!({ "a": 1 })),
    )
    .await;
    assert!(status.is_success(), "{body}");
    let (_, doc) = send(&app, get("/docs/_doc/2")).await;
    assert_eq!(doc["_source"]["env"], "prod", "{doc}");
}

/// A pipeline persisted by a build with NO gate still runs after the upgrade —
/// the compatibility door this marker narrows must stay open for the case it
/// was cut for.
///
/// Built the way `cluster_state.json` actually looked before the sweep: a
/// definition that COMPILED (so the translated `stages` body was stored),
/// carrying an option no build then checked, and with no marker file — which
/// is exactly the on-disk state an older binary leaves behind.
#[tokio::test]
async fn a_definition_with_no_refusal_marker_still_replays_softly() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = app_in(dir.path());
    let (status, body) = send(
        &app,
        put(
            "/_ingest/pipeline/legacy",
            json!({ "processors": [{ "set": { "field": "env", "value": "prod" } }] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    drop(app);

    let state_path = dir.path().join("cluster_state.json");
    let mut state: Value =
        serde_json::from_slice(&std::fs::read(&state_path).expect("cluster_state.json"))
            .expect("parse");
    state["pipelines"]["legacy"]["stages"][0]["config"]["ignore_failure"] = json!(true);
    std::fs::write(&state_path, serde_json::to_vec_pretty(&state).unwrap()).unwrap();
    assert!(
        !dir.path().join("ingest-pipeline-refusals.json").exists(),
        "a definition that compiled records no refusal, and older builds wrote no marker at all"
    );

    let app = app_in(dir.path());
    let (status, body) = send(
        &app,
        put(
            "/docs/_doc/1?pipeline=legacy&refresh=true",
            json!({ "a": 1 }),
        ),
    )
    .await;
    assert!(
        status.is_success(),
        "an unmarked definition must keep working across a binary upgrade: {body}"
    );
    let (_, doc) = send(&app, get("/docs/_doc/1")).await;
    assert_eq!(doc["_source"]["env"], "prod", "{doc}");

    // …and a live re-PUT of the same thing is still refused, so the door is
    // narrow and the repair path still answers.
    let (status, body) = send(
        &app,
        put(
            "/_ingest/pipeline/legacy",
            json!({ "processors": [
                { "set": { "field": "env", "value": "prod", "ignore_failure": true } }
            ] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = send(&app, put("/docs/_doc/2?pipeline=legacy", json!({ "a": 1 }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

// ─────────────────────────────────────────────────────────────────────────────
// `GET /_ingest/pipeline/{id}` speaks Elasticsearch.
//
// `_simulate` was taught the ES vocabulary in this branch; GET was not, so it
// handed back xerj's internal `stages` shape. That is what an operator edits
// and PUTs back, which is precisely how the restart hole above is reached.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn get_ingest_pipeline_answers_in_elasticsearch_vocabulary() {
    let (app, _dir) = app();
    let (status, body) = send(
        &app,
        put(
            "/_ingest/pipeline/p_rename",
            json!({ "description": "d",
                    "processors": [{ "rename": { "field": "a", "target_field": "b" } }] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (_, got) = send(&app, get("/_ingest/pipeline/p_rename")).await;
    assert_eq!(got["p_rename"]["description"], "d", "{got}");
    assert_eq!(
        got["p_rename"]["processors"][0]["rename"]["field"], "a",
        "GET must not leak the internal `stages` shape: {got}"
    );
    assert_eq!(
        got["p_rename"]["processors"][0]["rename"]["target_field"], "b",
        "{got}"
    );
    assert_eq!(
        got["p_rename"].get("stages"),
        None,
        "…and must not carry both shapes: {got}"
    );

    // `GET /_ingest/pipeline` (all) answers the same way.
    let (_, all) = send(&app, get("/_ingest/pipeline")).await;
    assert_eq!(
        all["p_rename"]["processors"][0]["rename"]["field"], "a",
        "{all}"
    );

    // Round trip: what GET hands back must PUT back cleanly and still run.
    let (status, body) = send(&app, put("/_ingest/pipeline/p2", got["p_rename"].clone())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = send(
        &app,
        put("/docs/_doc/1?pipeline=p2&refresh=true", json!({ "a": 1 })),
    )
    .await;
    assert!(status.is_success(), "{body}");
    let (_, doc) = send(&app, get("/docs/_doc/1")).await;
    assert_eq!(doc["_source"]["b"], 1, "{doc}");
    assert_eq!(doc.pointer("/_source/a"), None, "{doc}");
}

/// The ES→xerj `set` translation writes `override` into the stored config
/// because Elasticsearch's default is `true` and xerj's native default is
/// `false`. GET must therefore STATE it rather than echo a key the caller never
/// wrote without explanation — and it must round-trip, so that editing the
/// response and PUTting it back cannot silently flip the stage's behaviour.
/// (The native-stage half of this — `override: false` — is unit-tested on
/// `stage_to_es_processors` in `es_compat.rs`, since the native `PUT
/// /v1/pipelines/{name}` lives on the other router.)
#[tokio::test]
async fn get_states_the_override_the_stage_actually_applies() {
    let (app, _dir) = app();
    send(
        &app,
        put(
            "/_ingest/pipeline/es_set",
            json!({ "processors": [{ "set": { "field": "e", "value": "prod" } }] }),
        ),
    )
    .await;
    let (_, got) = send(&app, get("/_ingest/pipeline/es_set")).await;
    assert_eq!(
        got["es_set"]["processors"][0]["set"]["override"], true,
        "an ES `set` overrides, and GET says so: {got}"
    );

    // An explicit `override: false` survives the round trip and still
    // preserves the existing value.
    let (status, body) = send(
        &app,
        put(
            "/_ingest/pipeline/keeper",
            json!({ "processors": [
                { "set": { "field": "e", "value": "prod", "override": false } }
            ] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, got) = send(&app, get("/_ingest/pipeline/keeper")).await;
    assert_eq!(
        got["keeper"]["processors"][0]["set"]["override"], false,
        "{got}"
    );
    let (status, body) = send(
        &app,
        put("/_ingest/pipeline/keeper2", got["keeper"].clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = send(
        &app,
        put(
            "/docs/_doc/1?pipeline=keeper2&refresh=true",
            json!({ "e": "dev" }),
        ),
    )
    .await;
    assert!(status.is_success(), "{body}");
    let (_, doc) = send(&app, get("/docs/_doc/1")).await;
    assert_eq!(
        doc["_source"]["e"], "dev",
        "a re-PUT of GET's own output must not start overwriting: {doc}"
    );
}

// ── The create-time analysis gate must cover every spelling it accepts ───────

/// `PUT /{index}/_settings` refuses an `analysis` declaration in all four
/// spellings; index CREATION refused two of them and handed a `200` to the
/// byte-equivalent dotted one.
///
/// The create gate resolves the JSON pointers `/analysis` and `/index/analysis`
/// only, so `{"index.analysis.filter.my_lower.type": "lowercase"}` — a flat
/// dotted key, which this same handler already parses for `index.sort.field` —
/// found nothing to check, built no filter, and was echoed straight back by
/// `GET /{index}/_settings`. By this release's own words that declaration
/// "never lowercased anything": accepted and not honoured, which is the silent
/// lie the nested spelling was closed against.
#[tokio::test]
async fn every_spelling_of_an_unhonourable_analysis_block_is_refused_at_create() {
    let (app, _dir) = app();

    // The same declaration, written four ways. `my_lower` is a custom NAME
    // that resolves to no built-in, so none of them can be honoured.
    let nested = json!({
        "filter": { "my_lower": { "type": "lowercase" } },
        "analyzer": { "a": { "type": "custom", "tokenizer": "standard", "filter": ["my_lower"] } }
    });
    let bodies = [
        ("nested", json!({ "settings": { "analysis": nested } })),
        (
            "namespaced",
            json!({ "settings": { "index": { "analysis": nested } } }),
        ),
        (
            "dotted",
            json!({ "settings": {
                "index.analysis.filter.my_lower.type": "lowercase",
                "index.analysis.analyzer.a.type": "custom",
                "index.analysis.analyzer.a.tokenizer": "standard"
            } }),
        ),
        (
            "half-dotted",
            json!({ "settings": { "index": {
                "analysis.filter.my_lower.type": "lowercase",
                "analysis.analyzer.a.type": "custom"
            } } }),
        ),
    ];

    for (i, (spelling, body)) in bodies.iter().enumerate() {
        let (status, resp) = send(&app, put(&format!("/spelling_{i}"), body.clone())).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "the {spelling} spelling must be refused like the others: {resp}"
        );
        assert_eq!(
            resp["error"]["type"], "action_request_validation_exception",
            "{spelling}: {resp}"
        );
        assert!(
            resp["error"]["reason"]
                .as_str()
                .is_some_and(|r| r.contains("my_lower")),
            "{spelling}: the refusal must name the construct it cannot honour: {resp}"
        );
    }

    // And the gate stays narrow: a settings body that declares no analysis at
    // all still creates, and an unrelated dotted key does not trip it.
    let (status, resp) = send(
        &app,
        put(
            "/plain_idx",
            json!({ "settings": { "index.number_of_shards": 1, "index.sort.field": "ts" } }),
        ),
    )
    .await;
    assert!(
        status.is_success(),
        "an unrelated dotted setting is not an analysis declaration: {resp}"
    );
}

// ── A native definition read back in ES vocabulary must round-trip ───────────

/// `GET /_ingest/pipeline` is the only read endpoint for a pipeline created in
/// xerj's own `stages` vocabulary — the native surface registers no GET — and
/// this release taught it to answer in Elasticsearch's `processors` vocabulary.
/// PUTting that output back then went through the ES translation, which built a
/// fresh `{description, stages}` object and dropped everything else: a
/// pipeline's `on_error: "pass"` (keep the document) silently reverted to the
/// `Drop` default (discard it), and its `timeout_ms` vanished — under a
/// `200 {"acknowledged": true}`.
#[tokio::test]
async fn a_pipelines_error_policy_survives_a_round_trip_through_the_es_endpoint() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = app_in(dir.path());

    // A `stages`-shaped body takes the handler's pass-through branch, which is
    // exactly what `PUT /v1/pipelines/{name}` stores.
    let native = json!({
        "description": "n3",
        "on_error": "pass",
        "timeout_ms": 250,
        "stages": [{ "type": "set", "config": { "field": "e", "value": "prod" } }]
    });
    let (status, body) = send(&app, put("/_ingest/pipeline/n3", native)).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (_, stored) = send(&app, get("/_ingest/pipeline/n3")).await;
    assert_eq!(stored["n3"]["on_error"], "pass", "{stored}");
    assert_eq!(stored["n3"]["timeout_ms"], 250, "{stored}");

    // The operator PUTs back what GET handed them, unedited.
    let (status, body) = send(&app, put("/_ingest/pipeline/n3", stored["n3"].clone())).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (_, after) = send(&app, get("/_ingest/pipeline/n3")).await;
    assert_eq!(
        after["n3"]["on_error"], "pass",
        "measured before this fix: the key was gone and the policy reverted from `pass` \
         (keep the document) to the `Drop` default (discard it): {after}"
    );
    assert_eq!(after["n3"]["timeout_ms"], 250, "{after}");

    // The same must hold across a restart, since it is the stored definition
    // that the boot recompiles.
    drop(app);
    let app = app_in(dir.path());
    let (_, rebooted) = send(&app, get("/_ingest/pipeline/n3")).await;
    assert_eq!(rebooted["n3"]["on_error"], "pass", "{rebooted}");
    assert_eq!(rebooted["n3"]["timeout_ms"], 250, "{rebooted}");
}

/// Elasticsearch metadata on a pipeline that compiles is echoed back rather
/// than dropped — the same edit-do-not-rebuild rule, one level up.
#[tokio::test]
async fn es_pipeline_metadata_is_not_dropped_by_the_translation() {
    let (app, _dir) = app();
    let (status, body) = send(
        &app,
        put(
            "/_ingest/pipeline/meta",
            json!({
                "description": "d",
                "version": 3,
                "_meta": { "owner": "platform" },
                "processors": [{ "set": { "field": "e", "value": "prod" } }]
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, got) = send(&app, get("/_ingest/pipeline/meta")).await;
    assert_eq!(got["meta"]["version"], 3, "{got}");
    assert_eq!(got["meta"]["_meta"]["owner"], "platform", "{got}");
    assert_eq!(got["meta"]["description"], "d", "{got}");
    assert!(got["meta"].get("stages").is_none(), "{got}");
}

// ── Concurrency: the refusal marker under parallel writes ───────────────────

/// The HTTP statement of the engine-level race: refusals issued CONCURRENTLY
/// must all survive a restart. Every other restart test on this endpoint is
/// sequential, which is why a green suite sat on top of a mechanism that did
/// not hold.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrently_refused_pipelines_are_all_still_refused_after_a_restart() {
    const N: usize = 24;

    let dir = tempfile::tempdir().expect("tempdir");
    let app = app_in(dir.path());

    let mut tasks = Vec::with_capacity(N);
    for i in 0..N {
        let app = app.clone();
        tasks.push(tokio::spawn(async move {
            let (status, body) = send(
                &app,
                put(
                    &format!("/_ingest/pipeline/sc{i}"),
                    json!({ "stages": [{
                        "type": "drop_field",
                        "config": { "fields": ["secret"], "if": "ctx.tenant == 'a'" }
                    }] }),
                ),
            )
            .await;
            (i, status, body)
        }));
    }
    for t in tasks {
        let (i, status, body) = t.await.expect("no panic");
        assert_eq!(status, StatusCode::OK, "sc{i}: {body}");
        assert_eq!(body["acknowledged"], true, "sc{i}: {body}");
    }

    // Restart. Nothing else — no PUT, no operator action.
    drop(app);
    let app = app_in(dir.path());

    for i in 0..N {
        let (status, body) = send(
            &app,
            put(
                &format!("/docs/_doc/{i}?pipeline=sc{i}"),
                json!({ "tenant": "b", "secret": "ssn-111" }),
            ),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "sc{i} was refused before the restart and must still be refused after it — \
             measured before the flush was serialised: 3 of 40 markers never reached \
             disk, and the identical write answered 201 with `secret` dropped from a \
             document the guard excludes: {body}"
        );
        let (status, doc) = send(&app, get(&format!("/docs/_doc/{i}"))).await;
        assert_ne!(status, StatusCode::OK, "sc{i}: {doc}");
    }
}

/// A corrupt sidecar must not be silently repaired into a smaller one. Seen
/// from HTTP: the refusal that would have overwritten it is not acknowledged.
#[tokio::test]
async fn a_refused_put_is_not_acknowledged_while_the_sidecar_is_unreadable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ingest-pipeline-refusals.json");
    let corrupt = b"{\"older_refusal\": \"processor-level [if] is not supported\", ";
    std::fs::write(&path, corrupt).unwrap();

    let app = app_in(dir.path());
    let (status, body) = send(
        &app,
        put(
            "/_ingest/pipeline/newly_refused",
            json!({ "processors": [
                { "set": { "field": "e", "value": "prod", "ignore_failure": true } }
            ] }),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "a refusal that cannot be recorded must not be acknowledged: {body}"
    );
    assert!(
        body["error"]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("ingest-pipeline-refusals.json")),
        "and it must name the file to move aside: {body}"
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        corrupt,
        "the sidecar must be left exactly as found"
    );
}

// ── The native pipeline endpoint and the ES body it now hands out ───────────

fn native_app_in(data_dir: &std::path::Path) -> axum::Router {
    let mut config = xerj_common::config::Config::default();
    config.server.data_dir = data_dir.to_string_lossy().into_owned();
    config.storage.wal_sync = xerj_common::config::WalSync::Async;
    let metrics = xerj_common::metrics::Metrics::new().expect("metrics");
    let engine = xerj_engine::Engine::new(config.clone()).expect("engine");
    let state = xerj_api::state::AppState::new(config, engine, metrics);
    xerj_api::router::build_native_router(state)
}

/// `GET /_ingest/pipeline/{name}` is the only way to read a pipeline created
/// through `PUT /v1/pipelines/{name}`, and it now answers in Elasticsearch's
/// vocabulary. PUTting that body back to the native endpoint hit serde's
/// "missing field stages" and surfaced as a bare `500 internal error` — a
/// client-shaped mistake reported as an engine fault, with no hint of where the
/// body belongs.
#[tokio::test]
async fn the_native_endpoint_answers_400_and_names_the_endpoint_for_an_es_body() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = native_app_in(dir.path());

    let (status, body) = send(
        &app,
        put(
            "/v1/pipelines/n1",
            json!({ "description": "n1",
                    "processors": [{ "set": { "field": "e", "value": "prod" } }] }),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "measured before this fix: 500 internal error, missing field `stages`: {body}"
    );
    assert!(
        body["reason"]
            .as_str()
            .is_some_and(|r| r.contains("/_ingest/pipeline/n1")),
        "the error must name the endpoint that accepts the body: {body}"
    );

    // The native shape still works.
    let (status, body) = send(
        &app,
        put(
            "/v1/pipelines/n1",
            json!({ "stages": [{ "type": "set", "config": { "field": "e", "value": "prod" } }] }),
        ),
    )
    .await;
    assert!(status.is_success(), "{body}");
}
