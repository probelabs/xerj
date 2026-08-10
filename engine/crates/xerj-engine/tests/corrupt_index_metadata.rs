//! Issue #202 — an unparseable index sidecar must never be silently replaced
//! by a default.
//!
//! `schema.json` and `settings.json` are the only on-disk record of an index's
//! explicit mapping and analysis chain. Before this fix `Index::open` did
//!
//! ```ignore
//! let schema = load_schema(&index_dir).unwrap_or_else(|_| ManagedSchema::dynamic());
//! ```
//!
//! so a torn write, a bad disk or a truncated restore opened the index green
//! with every explicit field gone — and the next documents re-inferred the
//! field types from their own values. A `keyword` that was deliberately not
//! analyzed silently became whatever the first value looked like.
//!
//! ABSENT and UNPARSEABLE are different conditions. Absent is legitimate
//! (pre-0.6 indices never wrote `schema.json` at create time, and an index
//! created without settings has no `settings.json`) and must still open with a
//! dynamic mapping. Unparseable means the mapping is lost, and the index must
//! refuse to open so the operator sees red instead of quiet data corruption.

use serde_json::json;
use tempfile::TempDir;
use xerj_common::config::Config;
use xerj_common::types::{FieldConfig, FieldType, IndexName, Schema};
use xerj_engine::index::Index;
use xerj_engine::Engine;

fn config_for(dir: &TempDir) -> Config {
    let mut config = Config::default();
    config.server.data_dir = dir.path().to_str().unwrap().to_string();
    // `Index::create`/`Index::open` outside `Engine::new` still need the
    // process-wide governor; `init` is idempotent (first call wins).
    xerj_engine::governor::init(&config);
    config
}

/// An index whose mapping pins `code` as a `keyword` — the field whose meaning
/// silently changed when the schema was dropped on the floor.
fn mapped_schema() -> Schema {
    let mut schema = Schema::empty();
    schema
        .add_field(FieldConfig::new("code", FieldType::Keyword))
        .unwrap();
    schema
        .add_field(FieldConfig::new("body", FieldType::Text))
        .unwrap();
    schema
}

/// Create `name` with an explicit mapping, then drop it (closing the store).
async fn create_mapped_index(dir: &TempDir, name: &str) {
    let config = config_for(dir);
    let index = Index::create(
        IndexName::new(name).unwrap(),
        mapped_schema(),
        &config,
        dir.path(),
    )
    .unwrap();
    index
        .index_document(Some("d1".to_string()), json!({"code": "A-1", "body": "hi"}))
        .await
        .unwrap();
    index.flush().await.unwrap();
    drop(index);
}

// ── schema.json ───────────────────────────────────────────────────────────────

/// The core defect. Pre-fix this opened `Ok` with `field_count() == 0`.
#[tokio::test]
async fn unparseable_schema_json_refuses_to_open() {
    let dir = TempDir::new().unwrap();
    create_mapped_index(&dir, "torn_schema").await;

    // A torn write: the file exists, is non-empty, and is not valid JSON.
    let path = dir.path().join("torn_schema").join("schema.json");
    let good = std::fs::read(&path).unwrap();
    std::fs::write(&path, &good[..good.len() / 2]).unwrap();

    let config = config_for(&dir);
    let err = Index::open(IndexName::new("torn_schema").unwrap(), &config, dir.path())
        .err()
        .map(|e| e.to_string())
        .unwrap_or_else(|| {
            panic!(
                "opened an index whose schema.json is unparseable — the explicit mapping is gone"
            )
        });
    assert!(
        err.contains("schema.json"),
        "the error must name the file the operator has to restore, got: {err}"
    );
}

/// Semantically valid JSON that is not a `ManagedSchema` is just as lost as a
/// torn file — `serde_json::from_slice` fails either way and the old code
/// swallowed both.
#[tokio::test]
async fn wrong_shape_schema_json_refuses_to_open() {
    let dir = TempDir::new().unwrap();
    create_mapped_index(&dir, "wrong_shape").await;

    let path = dir.path().join("wrong_shape").join("schema.json");
    std::fs::write(&path, br#"{"schema": "not an object"}"#).unwrap();

    let config = config_for(&dir);
    assert!(
        Index::open(IndexName::new("wrong_shape").unwrap(), &config, dir.path()).is_err(),
        "a schema.json that does not deserialize must not fall back to a dynamic mapping"
    );
}

/// The other half of the contract: absent is not unparseable. Indices created
/// before create-time schema persistence have no `schema.json` at all and must
/// keep opening.
#[tokio::test]
async fn absent_schema_json_still_opens_dynamic() {
    let dir = TempDir::new().unwrap();
    create_mapped_index(&dir, "no_schema").await;
    std::fs::remove_file(dir.path().join("no_schema").join("schema.json")).unwrap();

    let config = config_for(&dir);
    let index = Index::open(IndexName::new("no_schema").unwrap(), &config, dir.path())
        .expect("an index without schema.json must still open with a dynamic mapping");
    assert_eq!(index.schema().await.field_count(), 0);
}

/// A mapping that parses must still round-trip — the fix must not turn a
/// healthy reopen into a failure.
#[tokio::test]
async fn intact_schema_json_still_opens_with_its_mapping() {
    let dir = TempDir::new().unwrap();
    create_mapped_index(&dir, "intact").await;

    let config = config_for(&dir);
    let index = Index::open(IndexName::new("intact").unwrap(), &config, dir.path())
        .expect("healthy reopen");
    let schema = index.schema().await;
    assert_eq!(
        schema.field("code").map(|f| f.field_type),
        Some(FieldType::Keyword),
        "the explicit keyword mapping must survive a restart"
    );
}

// ── settings.json ─────────────────────────────────────────────────────────────

/// `settings.json` carries the analysis chain and the WAL shard count. A
/// truncated one used to become `Value::Null`, which silently swaps every
/// custom analyzer for the default one — and changes the WAL layout the store
/// is opened with.
#[tokio::test]
async fn unparseable_settings_json_refuses_to_open() {
    let dir = TempDir::new().unwrap();
    {
        let config = config_for(&dir);
        let index = Index::create_with_settings(
            IndexName::new("torn_settings").unwrap(),
            mapped_schema(),
            json!({"index": {"number_of_replicas": 1, "refresh_interval": "5s"}}),
            &config,
            dir.path(),
        )
        .unwrap();
        drop(index);
    }

    let path = dir.path().join("torn_settings").join("settings.json");
    let good = std::fs::read(&path).unwrap();
    std::fs::write(&path, &good[..good.len() / 2]).unwrap();

    let config = config_for(&dir);
    let err = Index::open(
        IndexName::new("torn_settings").unwrap(),
        &config,
        dir.path(),
    )
    .err()
    .map(|e| e.to_string())
    .unwrap_or_else(|| panic!("opened an index whose settings.json is unparseable"));
    assert!(
        err.contains("settings.json"),
        "the error must name the file, got: {err}"
    );
}

/// An index created without settings has no `settings.json`; that must keep
/// opening.
#[tokio::test]
async fn absent_settings_json_still_opens() {
    let dir = TempDir::new().unwrap();
    create_mapped_index(&dir, "no_settings").await;
    assert!(
        !dir.path()
            .join("no_settings")
            .join("settings.json")
            .exists(),
        "Index::create with null settings writes no settings.json"
    );

    let config = config_for(&dir);
    Index::open(IndexName::new("no_settings").unwrap(), &config, dir.path())
        .expect("an index without settings.json must still open");
}

// ── es_mapping.json ───────────────────────────────────────────────────────────

/// The raw ES mapping blob (analyzers, date formats, `dense_vector` dims) is
/// the mapping users actually see through `GET /{index}/_mapping`. A corrupt
/// one used to be logged and ignored, leaving the index serving with a
/// silently emptier mapping than the one on disk.
#[tokio::test]
async fn unparseable_es_mapping_json_fails_the_index_on_startup() {
    let dir = TempDir::new().unwrap();
    create_mapped_index(&dir, "torn_es_mapping").await;
    std::fs::write(
        dir.path().join("torn_es_mapping").join("es_mapping.json"),
        b"{\"properties\": ",
    )
    .unwrap();

    let engine = Engine::new(config_for(&dir)).expect("engine::new");
    assert!(
        engine.get_index("torn_es_mapping").is_err(),
        "an index with a corrupt es_mapping.json must not be served"
    );
    assert!(
        engine.failed_indices.contains_key("torn_es_mapping"),
        "the failure must be recorded so cluster health goes red"
    );
    assert_eq!(engine.health().await.status, "red");
}

// ── the user-visible signal ───────────────────────────────────────────────────

/// End to end: a node booting over a corrupt `schema.json` must come up red
/// with the index unserved, not green with an empty mapping.
#[tokio::test]
async fn engine_startup_reports_red_for_a_corrupt_schema() {
    let dir = TempDir::new().unwrap();
    create_mapped_index(&dir, "torn_boot").await;
    let path = dir.path().join("torn_boot").join("schema.json");
    let good = std::fs::read(&path).unwrap();
    std::fs::write(&path, &good[..good.len() / 2]).unwrap();

    let engine = Engine::new(config_for(&dir)).expect("engine::new");
    assert!(
        engine.get_index("torn_boot").is_err(),
        "a corrupt index must not be served with a re-inferred mapping"
    );
    let reason = engine
        .failed_indices
        .get("torn_boot")
        .map(|r| r.value().reason.clone())
        .expect("the failure must be recorded in failed_indices");
    assert!(
        reason.contains("schema.json"),
        "the recorded reason must name the file, got: {reason}"
    );
    assert_eq!(engine.health().await.status, "red");
}

/// Refusing the open is only half the fix: a failed index is absent from
/// `indices`, so `PUT /{index}` and bulk auto-create used to sail straight past
/// the "already exists" check and run `Index::create` over the corrupt
/// directory — overwriting `schema.json` with an empty mapping and destroying
/// the evidence. That door has to be shut too.
#[tokio::test]
async fn create_over_a_failed_index_is_refused() {
    let dir = TempDir::new().unwrap();
    create_mapped_index(&dir, "no_recreate").await;
    let path = dir.path().join("no_recreate").join("schema.json");
    let good = std::fs::read(&path).unwrap();
    std::fs::write(&path, &good[..good.len() / 2]).unwrap();

    let engine = Engine::new(config_for(&dir)).expect("engine::new");
    let err = engine
        .create_index("no_recreate", Schema::empty())
        .expect_err("re-creating a failed index must not silently succeed")
        .to_string();
    assert!(
        err.contains("failed to open") && err.contains("DELETE"),
        "the error must explain the state and the way out, got: {err}"
    );
    // Auto-create (the bulk/index-a-document door) goes through the same gate.
    assert!(engine.get_or_create_index("no_recreate").is_err());
    // And the corrupt file is still there, untouched, for the operator.
    assert_eq!(
        std::fs::read(&path).unwrap().len(),
        good.len() / 2,
        "a refused create must not have rewritten schema.json"
    );
}

/// …and refusing to create must not brick the name. Delete is the operator's
/// recovery path (restore from backup afterwards), so it has to work on an
/// index that never opened.
#[tokio::test]
async fn a_failed_index_can_still_be_deleted() {
    let dir = TempDir::new().unwrap();
    create_mapped_index(&dir, "removable").await;
    let path = dir.path().join("removable").join("schema.json");
    let good = std::fs::read(&path).unwrap();
    std::fs::write(&path, &good[..good.len() / 2]).unwrap();

    let engine = Engine::new(config_for(&dir)).expect("engine::new");
    assert_eq!(engine.health().await.status, "red");

    engine
        .delete_index("removable")
        .await
        .expect("a failed index must be deletable — otherwise the name is bricked");
    assert!(
        !dir.path().join("removable").exists(),
        "the data directory must be gone"
    );
    assert!(!engine.failed_indices.contains_key("removable"));
    assert_eq!(
        engine.health().await.status,
        "green",
        "health must recover once the corrupt index is gone"
    );

    // The name is usable again.
    engine.create_index("removable", Schema::empty()).unwrap();
    assert!(engine.get_index("removable").is_ok());
}

// ── the writer must not manufacture what the reader now refuses ───────────────

/// Refusing a torn sidecar is only safe if our own writer cannot produce one.
///
/// `write_file_atomic` staged every write in `<file>.tmp` — one shared name for
/// all writers of that path. Two concurrent settings writes (`PUT /_settings`
/// racing another `PUT /_settings`, or an `index.blocks` update) therefore both
/// opened it `O_TRUNC` and interleaved their bytes before either renamed, so
/// the *complete* file that landed was a mix of two bodies. Pre-fix, this test
/// reports 294 of 800 writes failing with ENOENT (the loser renaming a file the
/// winner had already moved) and — with those errors swallowed, which is what
/// the callers do — 16 of 200 rounds leaving an unparseable file. After the
/// fix: no failed writes, no torn file, no debris.
///
/// Before #202 that torn file was "only" silently swapped for an empty mapping.
/// Now it refuses the open, so a lost race would brick the index — which is why
/// this test lives here and not in a general-hygiene file.
#[test]
fn concurrent_atomic_writes_never_leave_a_torn_sidecar() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("settings.json");

    // Two bodies of very different length: a short write landing inside a long
    // one is what leaves trailing garbage behind valid-looking JSON.
    let small = serde_json::to_vec_pretty(&json!({"index": {"refresh_interval": "1s"}})).unwrap();
    let big = serde_json::to_vec_pretty(&json!({
        "index": {"analysis": (0..400).map(|i| format!("filter-{i}")).collect::<Vec<_>>()}
    }))
    .unwrap();

    let mut torn = 0usize;
    let mut write_errors: Vec<String> = Vec::new();
    for _ in 0..200 {
        std::thread::scope(|s| {
            let handles: Vec<_> = [&small, &big, &small, &big]
                .into_iter()
                .map(|payload| {
                    let p = path.clone();
                    s.spawn(move || xerj_engine::index::write_file_atomic(&p, payload))
                })
                .collect();
            for h in handles {
                // A shared staging name also made writes fail spuriously: the
                // loser of the race renamed a file the winner had already moved
                // (ENOENT). Record it rather than panicking, so a failure names
                // the defect instead of "a scoped thread panicked".
                if let Err(e) = h.join().expect("writer thread") {
                    write_errors.push(e.to_string());
                }
            }
        });
        let bytes = std::fs::read(&path).unwrap();
        if serde_json::from_slice::<serde_json::Value>(&bytes).is_err() {
            torn += 1;
        }
    }
    assert!(
        write_errors.is_empty(),
        "{} concurrent writes failed, first: {} — writers must not collide on a staging file",
        write_errors.len(),
        write_errors[0]
    );
    assert_eq!(
        torn, 0,
        "{torn}/200 rounds of concurrent writes left a settings.json that does not parse — \
         the write path can manufacture the corruption the read path now refuses"
    );

    // And no staging debris is left behind for a successful write.
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "settings.json")
        .collect();
    assert!(
        leftovers.is_empty(),
        "staging files left behind: {leftovers:?}"
    );
}
