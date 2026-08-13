use xerj_common::config::{Config, WalSync};
use xerj_console_api::bootstrap;
use xerj_engine::{ClusterStateBlockKind, Engine};

fn config_for(path: &std::path::Path) -> Config {
    let mut config = Config::default();
    config.server.data_dir = path.to_string_lossy().into_owned();
    config.storage.wal_sync = WalSync::Async;
    config
}

fn snapshot(root: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    fn walk(root: &std::path::Path, path: &std::path::Path, out: &mut Vec<(String, Vec<u8>)>) {
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
                .to_string_lossy()
                .into_owned();
            let kind = entry.file_type().expect("snapshot file type");
            if kind.is_dir() {
                out.push((format!("d:{relative}"), Vec::new()));
                walk(root, &entry_path, out);
            } else if kind.is_symlink() {
                out.push((
                    format!("l:{relative}"),
                    std::fs::read_link(&entry_path)
                        .expect("read snapshot symlink")
                        .to_string_lossy()
                        .as_bytes()
                        .to_vec(),
                ));
            } else {
                out.push((
                    format!("f:{relative}"),
                    std::fs::read(&entry_path).expect("read snapshot file"),
                ));
            }
        }
    }

    let mut out = Vec::new();
    walk(root, root, &mut out);
    out
}

#[tokio::test]
async fn console_bootstrap_is_side_effect_free_when_cluster_state_is_blocked() {
    let dir = tempfile::tempdir().expect("tempdir");
    let future = include_bytes!(
        "../../xerj-engine/tests/fixtures/cluster_state/v2-format-version-minimal.json"
    );
    std::fs::write(dir.path().join("cluster_state.json"), future).expect("write future state");

    let engine = Engine::new(config_for(dir.path())).expect("blocked diagnostic engine");
    assert_eq!(
        engine.cluster_state_boot_status().block_kind(),
        Some(&ClusterStateBlockKind::UnsupportedFormat)
    );
    let before = snapshot(dir.path());

    let result = bootstrap::run(&engine, dir.path(), "http://127.0.0.1:9200").await;
    assert!(
        result.is_err(),
        "Console bootstrap must refuse a blocked storage node"
    );

    assert_eq!(snapshot(dir.path()), before);
    assert!(!dir.path().join(".xerj_master_key").exists());
    for system_index in xerj_console_api::indices::ALL {
        assert!(
            !dir.path().join(system_index).exists(),
            "must not create or open {system_index}"
        );
    }
}
