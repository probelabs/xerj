use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/failing-before-v1");
const EVIDENCE: &[u8] = include_bytes!("../testdata/failing-before-v1/evidence.json");
const MUTATIONS: &[u8] = include_bytes!("../testdata/review11-v1/mutations.json");

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Evidence {
    schema: String,
    subject: Subject,
    classification: Classification,
    normalization: Normalization,
    controls: Vec<Control>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Subject {
    commit: String,
    tree: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Classification {
    kind: String,
    upstream_comparison: String,
    reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Normalization {
    id: String,
    rules: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Control {
    id: String,
    patch: PatchArtifact,
    ledger_cases: Vec<String>,
    runs: Vec<Run>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Run {
    id: String,
    phase: String,
    command: String,
    expected_exit: String,
    observed_exit_code: i32,
    log: Artifact,
    required_substrings: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Artifact {
    path: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PatchArtifact {
    path: String,
    sha256: String,
    target_path: String,
    target_sha256: String,
}

struct ExpectedRun {
    control_id: &'static str,
    id: &'static str,
    phase: &'static str,
    command: &'static str,
    expected_exit: &'static str,
    observed_exit_code: i32,
    log_path: &'static str,
    log_sha256: &'static str,
    required_substrings: &'static [&'static str],
}

const EXPECTED_PATCHES: &[(&str, &str, &str, &str, &str)] = &[
    (
        "generation-sequence",
        "patches/generation-sequence.patch",
        "a3e826fbf65b7895a5526b4b24acce4b65bd78372316ce294dc623d8b92a072b",
        "src/plan.rs",
        "7e0a183a178b3ec94682648147aa9aed61b063fb6eabef1fba05b4acacd8b718",
    ),
    (
        "replay-cardinality",
        "patches/replay-cardinality.patch",
        "de2d97df38231e00c1db9c1c30e59c77ee138833ae69368cc7efe08dbcc8f05c",
        "src/plan.rs",
        "7e0a183a178b3ec94682648147aa9aed61b063fb6eabef1fba05b4acacd8b718",
    ),
    (
        "replay-operation-count",
        "patches/replay-operation-count.patch",
        "1ee3892e264b889ed9a2a1c246fb2320587523a23481b5e312306aab7c9989c8",
        "src/plan.rs",
        "7e0a183a178b3ec94682648147aa9aed61b063fb6eabef1fba05b4acacd8b718",
    ),
    (
        "historical-seven-case-ledger",
        "patches/historical-ledger.patch",
        "b4767fb694c63dc3ba2942850842d102ec54cca356bdace70f5bf659a28fa2e5",
        "testdata/review11-v1/mutations.json",
        "a8cef4d415680e9ebc49f4e723e300615bf55ac4a560e0836e0d772c3189a4da",
    ),
];

const EXPECTED_RUNS: &[ExpectedRun] = &[
    ExpectedRun { control_id: "generation-sequence", id: "mutated-before", phase: "mutated_before", command: "cargo test -p xerj-corpus-publication --test goldens generation_is_typed_and_independent_of_sequence -- --exact --nocapture", expected_exit: "nonzero", observed_exit_code: 101, log_path: "logs/generation-sequence.before.log", log_sha256: "1f7cf019aac847c8b847dc8c19965694b9d82dbea10bea32dd4778d86e57c595", required_substrings: &["assertion `left != right` failed", "DesiredPlanDigest(\"xerdp1-sha256-d89848a7c515772c16724522b3b0e27aca4fd0a871fa7cd4ea46ab9967956963\")", "test result: FAILED."] },
    ExpectedRun { control_id: "generation-sequence", id: "unmodified-after", phase: "unmodified_after", command: "cargo test -p xerj-corpus-publication --test goldens generation_is_typed_and_independent_of_sequence -- --exact --nocapture", expected_exit: "zero", observed_exit_code: 0, log_path: "logs/generation-sequence.after.log", log_sha256: "2383d74e857a733ba448888d1ef68dd47f182993b7e2c1007f75e3c18b392190", required_substrings: &["test generation_is_typed_and_independent_of_sequence ... ok", "test result: ok."] },
    ExpectedRun { control_id: "replay-cardinality", id: "removed-tuple-mutated-before", phase: "mutated_before", command: "cargo test -q -p xerj-corpus-publication --test replay_tuple_validation removed_replay_tuple_with_recomputed_digest_is_rejected_by_cardinality -- --exact", expected_exit: "nonzero", observed_exit_code: 101, log_path: "logs/replay-cardinality-removed.before.log", log_sha256: "10776098591f6b63f06d1c5cd8aea06958fb74841a568471fa1ffb4fa168dd19", required_substrings: &["left: \"mapping/resource/replay cardinalities do not match declared projections\"", "right: \"replay tuple cardinality does not match declared projections\"", "test result: FAILED."] },
    ExpectedRun { control_id: "replay-cardinality", id: "removed-tuple-unmodified-after", phase: "unmodified_after", command: "cargo test --locked -p xerj-corpus-publication --test replay_tuple_validation removed_replay_tuple_with_recomputed_digest_is_rejected_by_cardinality -- --exact --nocapture", expected_exit: "zero", observed_exit_code: 0, log_path: "logs/replay-cardinality-removed.after.log", log_sha256: "2c2d2f89092d830e5e887c92841c9e04311cfa6755e98dd64ff24afd311cf511", required_substrings: &["test removed_replay_tuple_with_recomputed_digest_is_rejected_by_cardinality ... ok", "test result: ok."] },
    ExpectedRun { control_id: "replay-cardinality", id: "extra-tuple-mutated-before", phase: "mutated_before", command: "cargo test -q -p xerj-corpus-publication --test replay_tuple_validation extra_internally_valid_replay_tuple_is_rejected -- --exact", expected_exit: "nonzero", observed_exit_code: 101, log_path: "logs/replay-cardinality-extra.before.log", log_sha256: "f5ae5ea909fc96bb33d28e26182175760cb96bd5f7c1572d67d6b90483d2dd52", required_substrings: &["left: \"mapping/resource/replay cardinalities do not match declared projections\"", "right: \"replay tuple cardinality does not match declared projections\"", "test result: FAILED."] },
    ExpectedRun { control_id: "replay-cardinality", id: "extra-tuple-unmodified-after", phase: "unmodified_after", command: "cargo test --locked -p xerj-corpus-publication --test replay_tuple_validation extra_internally_valid_replay_tuple_is_rejected -- --exact --nocapture", expected_exit: "zero", observed_exit_code: 0, log_path: "logs/replay-cardinality-extra.after.log", log_sha256: "f6644b43d0b97101e777b510a725921a6a899fcd8588a477df0c0f3279373203", required_substrings: &["test extra_internally_valid_replay_tuple_is_rejected ... ok", "test result: ok."] },
    ExpectedRun { control_id: "replay-operation-count", id: "mutated-before", phase: "mutated_before", command: "cargo test -q -p xerj-corpus-publication --test replay_tuple_validation replay_tuple_operation_counts_must_match_declared_projection_counts -- --exact", expected_exit: "nonzero", observed_exit_code: 101, log_path: "logs/replay-operation-count.before.log", log_sha256: "43b74792ed5160e89edacb0699e342697de0c387c9be7eb605852a1fbdb7d347", required_substrings: &["left: \"quota charge does not recompute from persisted plan bytes\"", "right: \"replay tuple operation count does not match declared projection count\"", "test result: FAILED."] },
    ExpectedRun { control_id: "replay-operation-count", id: "unmodified-after", phase: "unmodified_after", command: "cargo test --locked -p xerj-corpus-publication --test replay_tuple_validation replay_tuple_operation_counts_must_match_declared_projection_counts -- --exact --nocapture", expected_exit: "zero", observed_exit_code: 0, log_path: "logs/replay-operation-count.after.log", log_sha256: "4dc690b0eb1661258d1728e140c7e374a5269514ce5b1a516c6e2e47f2f89d32", required_substrings: &["test replay_tuple_operation_counts_must_match_declared_projection_counts ... ok", "test result: ok."] },
    ExpectedRun { control_id: "historical-seven-case-ledger", id: "historical-ledger-mutated-before", phase: "mutated_before", command: "cargo test --locked -p xerj-corpus-publication --test oracle_mutation_matrix mutation_inventory_is_concrete_unique_and_uses_closed_outcomes -- --exact --nocapture", expected_exit: "nonzero", observed_exit_code: 101, log_path: "logs/historical-ledger.before.log", log_sha256: "6fb0c488f5e5afd50df4011649f2c2330fca2525fd93f382a80ae869abded650", required_substrings: &["left: Null", "right: \"CrossFieldMismatch\"", "test result: FAILED."] },
    ExpectedRun { control_id: "historical-seven-case-ledger", id: "oracle-unmodified-after", phase: "unmodified_after", command: "cargo test --locked -p xerj-corpus-publication --test oracle_mutation_matrix -- --nocapture --test-threads=1", expected_exit: "zero", observed_exit_code: 0, log_path: "logs/historical-ledger-oracle.after.log", log_sha256: "5b6d12bb2248d6f24c7ed481fcd9bb1fd2ea9ad548d5ea26b15416fbb94001a1", required_substrings: &["test mutation_inventory_is_concrete_unique_and_uses_closed_outcomes ... ok", "test result: ok. 3 passed; 0 failed;"] },
    ExpectedRun { control_id: "historical-seven-case-ledger", id: "strict-replay-unmodified-after", phase: "unmodified_after", command: "cargo test --locked -p xerj-corpus-publication --test mutations strict_replay_content_join_matrix -- --exact --nocapture --test-threads=1", expected_exit: "zero", observed_exit_code: 0, log_path: "logs/historical-ledger-strict-replay.after.log", log_sha256: "f0deb4b93e8c548771f148830a83c03dc612cc2a5edbddde030b77eae32e9c54", required_substrings: &["test strict_replay_content_join_matrix ... ok", "test result: ok."] },
    ExpectedRun { control_id: "historical-seven-case-ledger", id: "cross-file-unmodified-after", phase: "unmodified_after", command: "cargo test --locked -p xerj-corpus-publication --test mutations persisted_cross_file_join_matrix -- --exact --nocapture --test-threads=1", expected_exit: "zero", observed_exit_code: 0, log_path: "logs/historical-ledger-cross-file.after.log", log_sha256: "33fd55af733b1dfa648ea9544cb19fed2e4be2bdc9a33d3504fdb3ecd69032bd", required_substrings: &["test persisted_cross_file_join_matrix ... ok", "test result: ok."] },
];

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn read_artifact(path: &str, expected_sha256: &str, kind: &str) -> Vec<u8> {
    assert!(
        path.starts_with(&format!("{kind}/")),
        "{path} is not a {kind} artifact",
    );
    assert!(!path.contains(".."), "parent traversal in artifact");
    assert_eq!(expected_sha256.len(), 64, "{path} hash length");
    assert!(
        expected_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "{path} hash is not lowercase SHA-256",
    );
    let bytes = std::fs::read(Path::new(ROOT).join(path)).unwrap();
    assert_eq!(sha256(&bytes), expected_sha256, "{path} hash");
    bytes
}

fn checked_in_artifact_paths() -> BTreeSet<String> {
    let mut paths = BTreeSet::new();
    for directory in ["logs", "patches"] {
        let root = Path::new(ROOT).join(directory);
        for entry in std::fs::read_dir(&root).unwrap() {
            let entry = entry.unwrap();
            assert!(
                entry.file_type().unwrap().is_file(),
                "nested artifact directory"
            );
            let relative = PathBuf::from(directory).join(entry.file_name());
            paths.insert(relative.to_string_lossy().replace('\\', "/"));
        }
    }
    paths
}

#[test]
fn checked_in_failing_before_evidence_is_closed_and_self_consistent() {
    let evidence: Evidence = serde_json::from_slice(EVIDENCE).unwrap();
    assert_eq!(evidence.schema, "xerj-failing-before-evidence/1");
    assert_eq!(
        evidence.subject.commit,
        "de1c63d90cbc4bc5a77faadf50220f1705f9ca37"
    );
    assert_eq!(
        evidence.subject.tree,
        "02390ea1fc5e9bb664614dbfb9f7a6507b0b3ce0"
    );
    assert_eq!(evidence.classification.kind, "deliberate_semantic_controls");
    assert_eq!(
        evidence.classification.upstream_comparison,
        "not_applicable"
    );
    assert_eq!(
        evidence.classification.reason,
        "The upstream base has no xerj-corpus-publication package; these controls mutate the exact final package and are not unmodified-upstream regressions."
    );
    assert_eq!(evidence.normalization.id, "cargo-test-log/1");
    assert_eq!(
        evidence.normalization.rules,
        [
            "decode as UTF-8 and convert CRLF or CR to LF",
            "remove ANSI CSI sequences",
            "discard all text before the first line matching ^running [0-9]+ tests?$",
            "replace only the numeric panic thread id with <THREAD>",
            "replace test-harness elapsed seconds with <DURATION>",
            "remove trailing horizontal whitespace and end with exactly one LF",
        ]
    );

    assert_eq!(evidence.controls.len(), EXPECTED_PATCHES.len());

    let mut control_ids = BTreeSet::new();
    let mut run_ids = BTreeSet::new();
    let mut declared_paths = BTreeSet::new();
    let mut ledger_cases = BTreeSet::new();
    let mut before_count = 0;
    let mut after_count = 0;

    let mut expected_runs = EXPECTED_RUNS.iter();
    for (control, expected_patch) in evidence.controls.iter().zip(EXPECTED_PATCHES) {
        assert_eq!(control.id, expected_patch.0);
        assert!(
            control_ids.insert(control.id.as_str()),
            "duplicate control id"
        );

        assert_eq!(control.patch.path, expected_patch.1);
        assert_eq!(control.patch.sha256, expected_patch.2);
        assert_eq!(control.patch.target_path, expected_patch.3);
        assert_eq!(control.patch.target_sha256, expected_patch.4);
        let target_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(&control.patch.target_path);
        assert_eq!(
            sha256(&std::fs::read(&target_path).unwrap()),
            control.patch.target_sha256,
            "{} preimage hash",
            control.patch.target_path
        );
        let patch = read_artifact(&control.patch.path, &control.patch.sha256, "patches");
        assert!(declared_paths.insert(control.patch.path.clone()));
        let patch = std::str::from_utf8(&patch).unwrap();
        let target = format!(
            "engine/crates/xerj-corpus-publication/{}",
            control.patch.target_path
        );
        assert!(patch.starts_with(&format!("diff --git a/{target} b/{target}\n")));
        assert!(patch.contains(&format!("+++ b/{target}\n")));

        for case in &control.ledger_cases {
            assert!(ledger_cases.insert(case.as_str()), "duplicate ledger case");
        }

        for run in &control.runs {
            let expected = expected_runs.next().expect("unexpected run");
            assert_eq!(expected.control_id, control.id);
            assert_eq!(run.id, expected.id);
            assert_eq!(run.phase, expected.phase);
            assert_eq!(run.command, expected.command);
            assert_eq!(run.expected_exit, expected.expected_exit);
            assert_eq!(run.observed_exit_code, expected.observed_exit_code);
            assert_eq!(run.log.path, expected.log_path);
            assert_eq!(run.log.sha256, expected.log_sha256);
            assert_eq!(
                run.required_substrings
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
                expected.required_substrings
            );
            assert!(
                run_ids.insert((control.id.as_str(), run.id.as_str())),
                "duplicate run id"
            );
            assert!(run.command.starts_with("cargo test "));
            assert!(!run.required_substrings.is_empty());
            assert!(declared_paths.insert(run.log.path.clone()));
            let log = read_artifact(&run.log.path, &run.log.sha256, "logs");
            let log = std::str::from_utf8(&log).unwrap();
            assert!(log.starts_with("running "), "{} prefix", run.log.path);
            assert!(log.ends_with('\n'), "{} terminal LF", run.log.path);
            assert!(!log.contains('\r'), "{} contains CR", run.log.path);
            assert!(!log.contains("\u{1b}["), "{} contains ANSI", run.log.path);
            assert!(log.contains("finished in <DURATION>"));
            for required in &run.required_substrings {
                assert!(
                    log.contains(required),
                    "{} is missing exact substring {required:?}",
                    run.log.path
                );
            }
            match (run.phase.as_str(), run.expected_exit.as_str()) {
                ("mutated_before", "nonzero") => {
                    before_count += 1;
                    assert_ne!(run.observed_exit_code, 0);
                    assert!(log.contains("test result: FAILED."));
                }
                ("unmodified_after", "zero") => {
                    after_count += 1;
                    assert_eq!(run.observed_exit_code, 0);
                    assert!(log.contains("test result: ok."));
                    assert!(!log.contains("FAILED"));
                }
                other => panic!("invalid phase/exit classification: {other:?}"),
            }
        }
    }
    assert!(expected_runs.next().is_none(), "missing expected run");
    assert_eq!(before_count, 5);
    assert_eq!(after_count, 7);
    assert_eq!(declared_paths, checked_in_artifact_paths());

    let expected_cases = BTreeSet::from([
        "data:changed-source-with-old-content-digest",
        "data:changed-path-with-old-content-and-artifact-digests",
        "begin-expected-owner-mismatch-plan-owner",
        "begin-expected-sequence-mismatch-plan-predecessor",
        "begin-publication-root-changed-owner-root-prefix-join-rejects",
        "begin-publication-prefix-changed-owner-root-prefix-join-rejects",
        "begin-publication-incarnation-changed-data-route-name-join-rejects",
    ]);
    assert_eq!(ledger_cases, expected_cases);
    assert_eq!(
        sha256(MUTATIONS),
        "a8cef4d415680e9ebc49f4e723e300615bf55ac4a560e0836e0d772c3189a4da"
    );
    let mutations: serde_json::Value = serde_json::from_slice(MUTATIONS).unwrap();
    let rows = mutations["rows"].as_array().unwrap();
    let declared_cases = rows
        .iter()
        .flat_map(|row| row["cases"].as_array().unwrap())
        .map(|case| case.as_str().unwrap())
        .collect::<BTreeSet<_>>();
    let attested_cases = rows
        .iter()
        .filter_map(|row| row.get("case_expectations"))
        .flat_map(|value| value.as_object().unwrap().keys())
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    assert!(expected_cases.is_subset(&declared_cases));
    assert!(expected_cases.is_subset(&attested_cases));

    for relative in declared_paths {
        let bytes = std::fs::read(Path::new(ROOT).join(relative)).unwrap();
        let text = std::str::from_utf8(&bytes).unwrap();
        for forbidden in [
            "/workspace/",
            "/tmp/",
            "controls/results/",
            ".raw.log",
            "generated_at",
            "2026-",
        ] {
            assert!(!text.contains(forbidden), "artifact contains {forbidden:?}");
        }
    }
}
