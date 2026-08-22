//! Issue #382 (end-to-end): the schema-evolution throttle must not suppress the
//! evolve call a value-shape transition needs.
//!
//! The single-doc ingest path caches a hash of each document's shape
//! (`hash_all_field_names`) and skips `evolve_schema_from_doc` while it is
//! unchanged, re-checking only every ~100 docs. A `<base>_chunks` companion
//! (`base` declared `dense_vector`) first seen as a rectangular numeric
//! multi-vector `[[..]]` is *refused* a keyword mapping; when the SAME key later
//! arrives as a flat string array it is a legitimately-new keyword field. While
//! the throttle hashed an array as one opaque leaf, both documents hashed
//! identically, the evolve was skipped, and a `term` query answered 0 hits for
//! up to ~100 docs (self-healing on throttle expiry — corpus-size-dependent,
//! exactly the class of bug the unit hash tests pin one layer down).
//!
//! Fixed by recursing the throttle fingerprint into array elements, so the
//! array-of-numbers -> array-of-strings transition changes the hash and forces
//! the evolve on the very next document. This test drives the real ingest +
//! search path (not the hash primitive) to guard the observable symptom.

use serde_json::{json, Value};
use tempfile::TempDir;
use xerj_common::config::Config;
use xerj_common::types::{FieldConfig, FieldType, Schema};
use xerj_engine::{Engine, Index};
use xerj_query::parse_request;

fn make_engine(dir: &TempDir) -> Engine {
    let mut config = Config::default();
    config.server.data_dir = dir.path().to_str().unwrap().to_string();
    Engine::new(config).expect("engine::new")
}

fn req(q: Value) -> xerj_query::ast::SearchRequest {
    parse_request(&json!({ "query": q, "size": 50 })).expect("parse_request")
}

async fn hit_ids(idx: &Index, q: Value) -> Vec<String> {
    idx.search(&req(q))
        .await
        .unwrap()
        .hits
        .iter()
        .map(|h| h.id.clone())
        .collect()
}

/// A `<base>_chunks` companion (base declared `dense_vector`) first seen as a
/// multi-vector `[[..numbers..]]` is refused a keyword mapping; when it later
/// arrives as a flat string array under the SAME key, the schema-evolution
/// throttle must let the evolve through so the string registers and is
/// queryable immediately — not suppress it for ~100 docs (#382).
#[tokio::test]
async fn refused_multivector_companion_then_string_array_is_searchable_immediately() {
    let dir = TempDir::new().unwrap();
    let engine = make_engine(&dir);

    // Declare `passages` dense_vector so `passages_chunks` is recognised as its
    // (refused) multi-vector companion — the value-shape refusal #382 is about.
    let mut schema = Schema::empty();
    let mut vec_field = FieldConfig::new("passages", FieldType::Vector);
    vec_field.options.dimensions = Some(3);
    schema.fields.push(vec_field);
    engine.create_index("mv", schema).unwrap();
    let idx = engine.get_index("mv").unwrap();

    // Doc "a": passages_chunks as a rectangular numeric multi-vector -> refused a
    // keyword FieldConfig, and it seeds the throttle's cached shape fingerprint.
    idx.index_document(
        Some("a".into()),
        json!({ "passages_chunks": [[0.1, 0.2, 0.3]] }),
    )
    .await
    .unwrap();

    // Doc "b": passages_chunks now a FLAT STRING array (not a multi-vector) ->
    // evolution wants to register it as a keyword. Same key, different element
    // shape: array-of-numbers -> array-of-strings, the transition the opaque-leaf
    // hash could not see.
    idx.index_document(
        Some("b".into()),
        json!({ "passages_chunks": ["searchable-tenant"] }),
    )
    .await
    .unwrap();

    // The string under the once-refused key must be found — pre-flush (memtable)
    // and post-flush (segment). On the pre-fix throttle both docs hashed the same
    // (array = opaque leaf), the evolve was skipped, and this returned {} for up
    // to 100 docs.
    let q = json!({ "term": { "passages_chunks": "searchable-tenant" } });
    assert_eq!(
        hit_ids(&idx, q.clone()).await,
        vec!["b".to_string()],
        "PRE-flush: a string array under a once-refused multi-vector companion must \
         register immediately, not stay throttle-suppressed (#382)"
    );

    idx.flush().await.unwrap();
    assert_eq!(
        hit_ids(&idx, q).await,
        vec!["b".to_string()],
        "POST-flush: the once-refused companion's string value stays searchable"
    );
}
