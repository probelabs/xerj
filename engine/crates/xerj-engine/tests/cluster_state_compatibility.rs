use sha2::{Digest, Sha256};
use xerj_common::config::{Config, WalSync};
use xerj_common::types::Schema;
use xerj_common::XerjError;
use xerj_engine::{ClusterStateBlockKind, ClusterStateBootStatus, Engine};

/// Byte-level storage snapshot. Directory and symlink topology is included,
/// and every regular file is represented by its length and SHA-256. This is
/// intentionally stronger than an index-count assertion: WAL replay can rotate
/// one file into another while leaving both the document count and directory
/// count unchanged.
fn recursive_storage_snapshot(root: &std::path::Path) -> Vec<String> {
    fn walk(root: &std::path::Path, path: &std::path::Path, out: &mut Vec<String>) {
        let mut entries: Vec<_> = std::fs::read_dir(path)
            .expect("read snapshot directory")
            .map(|entry| entry.expect("read snapshot entry"))
            .collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let entry_path = entry.path();
            let relative = entry_path
                .strip_prefix(root)
                .expect("snapshot path under root")
                .to_string_lossy();
            let kind = entry.file_type().expect("snapshot file type");
            if kind.is_dir() {
                out.push(format!("d:{relative}"));
                walk(root, &entry_path, out);
            } else if kind.is_symlink() {
                out.push(format!(
                    "l:{relative}->{}",
                    std::fs::read_link(&entry_path)
                        .expect("read snapshot symlink")
                        .to_string_lossy()
                ));
            } else {
                let bytes = std::fs::read(&entry_path).expect("read snapshot file");
                out.push(format!(
                    "f:{relative}:{}:{}",
                    bytes.len(),
                    hex_digest(&bytes)
                ));
            }
        }
    }

    let mut out = Vec::new();
    walk(root, root, &mut out);
    out
}

fn config_for(path: &std::path::Path) -> Config {
    let mut config = Config::default();
    config.server.data_dir = path.to_string_lossy().into_owned();
    config.storage.wal_sync = WalSync::Async;
    config
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/cluster_state")
            .join(name),
    )
    .expect("read fixture")
}

fn write_fixture(path: &std::path::Path, name: &str) -> Vec<u8> {
    let bytes = fixture(name);
    std::fs::write(path.join("cluster_state.json"), &bytes).expect("write fixture");
    bytes
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn cluster_state_format1_fixture_manifest_is_exact() {
    let manifest: serde_json::Value = serde_json::from_slice(&fixture("MANIFEST.json"))
        .expect("parse cluster-state fixture manifest");
    assert_eq!(manifest["schema"], "xerj.cluster-state-format1-fixtures.v1");
    assert_eq!(
        manifest["format1_writer_commit"],
        "ee29361bccce77de527baac3fe6ed387cd1f6dff"
    );
    assert_eq!(
        manifest["format1_writer_tree"],
        "7227e51db69f9910a355bfd6b30b6a64b52dac87"
    );
    assert_eq!(
        manifest["provenance"]["method"],
        "runtime-capture-through-public-apis"
    );
    assert_eq!(
        manifest["provenance"]["writer_source"],
        "engine/crates/xerj-engine/src/engine.rs"
    );
    assert_eq!(manifest["provenance"]["reproduction"], "REPRODUCE.txt");
    let reproduction = fixture("REPRODUCE.txt");
    assert_eq!(
        hex_digest(&reproduction),
        manifest["provenance"]["reproduction_sha256"]
            .as_str()
            .unwrap()
    );
    assert!(reproduction
        .windows("/_index_template/logs".len())
        .any(|window| window == b"/_index_template/logs"));
    for (name, expected) in manifest["fixtures"]
        .as_object()
        .expect("fixture manifest object")
    {
        let bytes = fixture(name);
        assert_eq!(
            bytes.len() as u64,
            expected["bytes"].as_u64().unwrap(),
            "{name}"
        );
        assert_eq!(
            hex_digest(&bytes),
            expected["sha256"].as_str().unwrap(),
            "{name}"
        );
    }
}

#[tokio::test]
async fn cluster_state_format1_valid_v1_is_stable_and_remains_v1_after_mutation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let original = write_fixture(dir.path(), "v1-ee29361-all-fields.json");

    {
        let engine = Engine::new(config_for(dir.path())).expect("open exact v1");
        assert_eq!(
            engine.cluster_state_boot_status(),
            &ClusterStateBootStatus::LoadedV1
        );
        assert_eq!(engine.templates.len(), 1);
        assert_eq!(engine.legacy_templates.len(), 1);
        assert_eq!(engine.component_templates.len(), 1);
        assert_eq!(engine.pipelines.len(), 1);
        assert_eq!(engine.data_streams.len(), 1);
        assert_eq!(engine.ilm_policies.len(), 1);
    }
    assert_eq!(
        std::fs::read(dir.path().join("cluster_state.json")).unwrap(),
        original,
        "a no-op restart must not normalize valid v1 bytes"
    );

    {
        let engine = Engine::new(config_for(dir.path())).expect("reopen exact v1");
        engine
            .put_component_template(
                "extra".to_owned(),
                serde_json::json!({"template":{"settings":{"refresh_interval":"1s"}}}),
            )
            .expect("intentional supported mutation");
    }
    let written: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("cluster_state.json")).unwrap())
            .unwrap();
    assert_eq!(written["version"], 1);
    assert_eq!(written.as_object().unwrap().len(), 7);
    for field in [
        "index_templates",
        "legacy_templates",
        "component_templates",
        "pipelines",
        "data_streams",
        "ilm_policies",
    ] {
        assert!(written[field].is_object(), "missing v1 map {field}");
    }
    assert_eq!(written["component_templates"].as_object().unwrap().len(), 2);
    assert_eq!(
        written["index_templates"]["logs"],
        serde_json::json!({
            "index_patterns": ["logs-*"],
            "settings": {"number_of_replicas": 0},
            "mappings": {"properties": {"host": {"type": "keyword"}}},
            "priority": 200
        })
    );
    assert_eq!(
        written["legacy_templates"]["legacy-logs"],
        serde_json::json!({
            "index_patterns": ["legacy-*"],
            "settings": {"index.number_of_shards": "1"}
        })
    );
    assert_eq!(
        written["component_templates"]["base-settings"],
        serde_json::json!({"template": {"settings": {"number_of_replicas": 1}}})
    );
    assert_eq!(
        written["component_templates"]["extra"],
        serde_json::json!({"template": {"settings": {"refresh_interval": "1s"}}})
    );
    assert_eq!(
        written["pipelines"]["tagger"],
        serde_json::json!({
            "description": "tag every doc",
            "stages": [{"type": "set", "config": {"field": "tagged", "value": "yes"}}]
        })
    );
    assert_eq!(
        written["data_streams"]["metrics-app"],
        serde_json::json!({
            "name": "metrics-app",
            "backing_indices": [".ds-metrics-app-000001"],
            "timestamp_field": "@timestamp",
            "generation": 1
        })
    );
    assert_eq!(
        written["ilm_policies"]["hot-warm"],
        serde_json::json!({"policy": {"phases": {"hot": {"actions": {}}}}})
    );

    let engine = Engine::new(config_for(dir.path())).expect("reopen mutated v1");
    assert_eq!(engine.templates.len(), 1);
    assert_eq!(engine.legacy_templates.len(), 1);
    assert_eq!(engine.component_templates.len(), 2);
    assert_eq!(engine.pipelines.len(), 1);
    assert_eq!(engine.data_streams.len(), 1);
    assert_eq!(engine.ilm_policies.len(), 1);
    assert_eq!(
        engine.templates.get("logs").unwrap().index_patterns,
        vec!["logs-*".to_owned()]
    );
    assert_eq!(
        engine
            .data_streams
            .get("metrics-app")
            .unwrap()
            .backing_indices,
        vec![".ds-metrics-app-000001".to_owned()]
    );
}

#[tokio::test]
async fn cluster_state_format1_incompatible_fixtures_latch_and_preserve_bytes() {
    let cases = [
        (
            "v1-unknown-top-level.json",
            ClusterStateBlockKind::IncompatibleSchema,
        ),
        (
            "v1-unknown-index-template-field.json",
            ClusterStateBlockKind::IncompatibleSchema,
        ),
        (
            "v1-unknown-data-stream-field.json",
            ClusterStateBlockKind::IncompatibleSchema,
        ),
        (
            "v1-duplicate-version.json",
            ClusterStateBlockKind::DuplicateKey,
        ),
        (
            "v1-duplicate-nested-value-key.json",
            ClusterStateBlockKind::DuplicateKey,
        ),
        (
            "v2-format-version-minimal.json",
            ClusterStateBlockKind::UnsupportedFormat,
        ),
    ];

    for (name, expected_kind) in cases {
        let dir = tempfile::tempdir().expect("tempdir");
        let original = write_fixture(dir.path(), name);
        let original_digest = hex_digest(&original);
        for boot in 1..=2 {
            let engine = Engine::new(config_for(dir.path())).expect("blocked engine still boots");
            assert_eq!(
                engine.cluster_state_boot_status().block_kind(),
                Some(&expected_kind),
                "{name}, boot {boot}"
            );
            let err = engine
                .put_component_template("must-not-land".to_owned(), serde_json::json!({}))
                .expect_err("blocked state must refuse mutation");
            let common: XerjError = err.into();
            assert!(matches!(&common, XerjError::ClusterStateUnavailable));
            drop(engine);

            let after = std::fs::read(dir.path().join("cluster_state.json")).unwrap();
            assert_eq!(after, original, "{name}, boot {boot}");
            assert_eq!(hex_digest(&after), original_digest, "{name}, boot {boot}");
            assert!(
                !dir.path().join("cluster_state.corrupt.json").exists(),
                "unsupported, unknown and duplicate-keyed bytes are not malformed salvage"
            );
        }
    }
}

#[tokio::test]
async fn cluster_state_format1_malformed_bytes_are_untouched_and_never_authorize_write() {
    let dir = tempfile::tempdir().expect("tempdir");
    let malformed = br#"{"version":1,"index_templates":{"cut":true"#;
    std::fs::write(dir.path().join("cluster_state.json"), malformed).unwrap();
    for boot in 1..=2 {
        let engine = Engine::new(config_for(dir.path())).expect("blocked engine still boots");
        assert_eq!(
            engine.cluster_state_boot_status().block_kind(),
            Some(&ClusterStateBlockKind::MalformedJson),
            "boot {boot}"
        );
        assert!(engine
            .put_component_template("must-not-land".to_owned(), serde_json::json!({}))
            .is_err());
        drop(engine);
        assert_eq!(
            std::fs::read(dir.path().join("cluster_state.json")).unwrap(),
            malformed,
            "boot {boot}"
        );
        assert!(
            !dir.path().join("cluster_state.corrupt.json").exists(),
            "a blocked diagnostic boot must not create recovery files"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn cluster_state_format1_read_failure_is_not_mislabeled_or_salvaged() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().expect("tempdir");
    let state_path = dir.path().join("cluster_state.json");
    symlink("cluster_state.json", &state_path).expect("create self-loop");
    let engine = Engine::new(config_for(dir.path())).expect("blocked engine still boots");
    assert_eq!(
        engine.cluster_state_boot_status().block_kind(),
        Some(&ClusterStateBlockKind::ReadFailure)
    );
    assert!(engine
        .put_component_template("must-not-land".to_owned(), serde_json::json!({}))
        .is_err());
    drop(engine);
    assert_eq!(
        std::fs::read_link(&state_path).expect("read self-loop"),
        std::path::PathBuf::from("cluster_state.json")
    );
    assert!(
        !dir.path().join("cluster_state.corrupt.json").exists(),
        "read failures are never parse-corruption salvage"
    );
}

#[tokio::test]
async fn cluster_state_format1_future_state_cannot_reconcile_and_downgrade_on_boot() {
    let dir = tempfile::tempdir().expect("tempdir");
    {
        let engine = Engine::new(config_for(dir.path())).expect("fresh engine");
        for name in ["user-events", ".xerj_dashboards", ".ds-events-000002"] {
            engine
                .create_index(name, Schema::empty())
                .unwrap_or_else(|err| panic!("create {name}: {err}"));
            engine
                .get_index(name)
                .expect("new index is open")
                .index_document(
                    Some("pending-wal".to_owned()),
                    serde_json::json!({"body": format!("unflushed {name}")}),
                )
                .await
                .unwrap_or_else(|err| panic!("append {name} WAL: {err}"));
        }
    }
    let original = write_fixture(dir.path(), "v2-format-version-minimal.json");
    let before = recursive_storage_snapshot(dir.path());

    for boot in 1..=2 {
        let engine = Engine::new(config_for(dir.path())).expect("blocked engine still boots");
        assert_eq!(
            engine.cluster_state_boot_status().block_kind(),
            Some(&ClusterStateBlockKind::UnsupportedFormat)
        );
        assert!(engine.data_streams.is_empty());
        assert!(engine.index_name_list().is_empty(), "boot {boot}");
        let get_err: XerjError = match engine.get_index("user-events") {
            Ok(_) => panic!("blocked boot must not open a user index"),
            Err(err) => err.into(),
        };
        assert!(matches!(get_err, XerjError::ClusterStateUnavailable));
        let create_err: XerjError = engine
            .create_index("must-not-exist", Schema::empty())
            .expect_err("blocked boot must not activate a new WAL")
            .into();
        assert!(matches!(create_err, XerjError::ClusterStateUnavailable));
        engine.audit.append(
            "blocked-diagnostic",
            "test",
            "cluster-state",
            "refused",
            "must remain memory-only",
        );
        drop(engine);

        assert_eq!(
            recursive_storage_snapshot(dir.path()),
            before,
            "blocked boot {boot} must not open, replay, rotate or flush any user/system index, WAL, segment, or durable audit file"
        );
        assert!(!dir.path().join("must-not-exist").exists());
    }
    assert_eq!(
        std::fs::read(dir.path().join("cluster_state.json")).unwrap(),
        original,
        "startup index discovery must not downgrade future cluster state"
    );
}

#[tokio::test]
async fn cluster_state_format1_direct_durable_mutators_fail_before_any_side_effect() {
    fn unavailable<T>(result: xerj_engine::Result<T>, operation: &str) {
        let error = match result {
            Ok(_) => panic!("{operation}"),
            Err(error) => error,
        };
        let common: XerjError = error.into();
        assert!(
            matches!(common, XerjError::ClusterStateUnavailable),
            "{operation}: {common}"
        );
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let future = write_fixture(dir.path(), "v2-format-version-minimal.json");
    let engine = Engine::new(config_for(dir.path())).expect("blocked engine");
    let policy: xerj_engine::lifecycle::LifecyclePolicy =
        serde_json::from_value(serde_json::json!({
            "default_state": "hot",
            "states": [{"name":"hot","actions":[],"transitions":[]}]
        }))
        .unwrap();
    engine.index_mappings.insert(
        "seed".to_owned(),
        serde_json::json!({"properties":{"kept":{"type":"keyword"}}}),
    );
    engine.api_keys.insert(
        "seed-key".to_owned(),
        xerj_engine::engine::ApiKeyRecord::new("seed", "secret", 1, None, Vec::new()),
    );
    engine
        .ism_policies
        .insert("seed-policy".to_owned(), policy.clone());
    engine.managed_indices.insert(
        "seed".to_owned(),
        xerj_engine::lifecycle::ManagedIndexState::new(
            "seed-policy".to_owned(),
            "hot".to_owned(),
            1,
        ),
    );
    engine.index_settings.insert(
        "seed".to_owned(),
        serde_json::json!({"index":{"lifecycle":{"name":"seed-policy"}}}),
    );
    engine
        .aliases
        .insert("seed-alias".to_owned(), vec!["seed".to_owned()]);

    let before_storage = recursive_storage_snapshot(dir.path());
    unavailable(
        engine.put_index_mapping("seed", serde_json::json!({"replaced":true})),
        "mapping mutation must be fenced",
    );
    unavailable(
        engine.persist_api_key(
            "new-key".to_owned(),
            xerj_engine::engine::ApiKeyRecord::new("new", "secret", 2, None, Vec::new()),
        ),
        "API-key insert must be fenced",
    );
    unavailable(
        engine
            .invalidate_api_keys(&["seed-key".to_owned()], 3)
            .map(|_| ()),
        "API-key invalidation must be fenced",
    );
    unavailable(
        engine.put_ism_policy("new-policy".to_owned(), policy),
        "ISM insert must be fenced",
    );
    unavailable(
        engine.remove_ism_policy("seed-policy").map(|_| ()),
        "ISM remove must be fenced",
    );
    unavailable(
        engine.persist_managed_indices(),
        "managed lifecycle persistence must be fenced",
    );
    unavailable(
        engine.detach_lifecycle("seed").map(|_| ()),
        "lifecycle detach must be fenced",
    );
    unavailable(
        engine.add_alias("new-alias", "seed"),
        "alias insert must be fenced",
    );
    unavailable(
        engine.remove_alias("seed-alias", "seed"),
        "alias remove must be fenced",
    );

    assert_eq!(engine.index_mappings.len(), 1);
    assert_eq!(
        engine.index_mappings.get("seed").unwrap().value(),
        &serde_json::json!({"properties":{"kept":{"type":"keyword"}}})
    );
    assert_eq!(engine.api_keys.len(), 1);
    assert!(!engine.api_keys.get("seed-key").unwrap().invalidated);
    assert_eq!(engine.ism_policies.len(), 1);
    assert!(engine.ism_policies.contains_key("seed-policy"));
    assert_eq!(engine.managed_indices.len(), 1);
    assert!(engine.managed_indices.contains_key("seed"));
    assert!(engine.lifecycle_detached.is_empty());
    assert_eq!(
        engine.index_settings.get("seed").unwrap().value(),
        &serde_json::json!({"index":{"lifecycle":{"name":"seed-policy"}}})
    );
    assert_eq!(engine.aliases.len(), 1);
    assert_eq!(
        engine.aliases.get("seed-alias").unwrap().value(),
        &vec!["seed".to_owned()]
    );
    assert_eq!(recursive_storage_snapshot(dir.path()), before_storage);
    assert_eq!(
        std::fs::read(dir.path().join("cluster_state.json")).unwrap(),
        future
    );
}
