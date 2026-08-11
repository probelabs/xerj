mod support;

#[path = "support/mutation_plan.rs"]
mod mutation_plan;

use std::{collections::BTreeSet, str::FromStr};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::Value;
use sha2::{Digest, Sha256};
use support::{absent_bundle, absent_bundle_with, present_bundle};
use xerj_corpus_publication::{
    CatalogMappingV1, CorpusIncarnationId, CorpusOwnerId, CorpusPublicationV1, DesiredPlanDigest,
    DesiredPublicationPlanV1, ExpectedPublicationDigest, ExpectedPublicationV1,
    ExtractorConfigDigest, ExtractorConfigV1, GraphEdgeMappingV1, GraphNodeMappingV1,
    ManifestDigest, ManifestV1, MappingDigest, PersistedDesiredPlanBytesV1,
    PersistedPreparedInputBytesV1, PersistedReplayArtifactBytesV1, PersistedSyncBeginBytesV1,
    PreparedInputDigest, PreparedInputV1, ProtocolError, ProtocolErrorKind, PublicationDigest,
    ReplayArtifactDigest, ReplaySetDigest, SyncBeginDigest, SyncBeginV1, TransactionId,
};

const MATRIX: &[u8] = include_bytes!("../testdata/review11-v1/mutations.json");
const GOLDENS: &[u8] = include_bytes!("../testdata/review11-v1/goldens.json");

type Runner = fn();

const INTEGRATION_RUNNERS: &[(&str, Runner)] = &[
    (
        "mutations::rendered_digest_cartesian",
        rendered_digest_cartesian,
    ),
    ("mutations::closed_json_cartesian", closed_json_cartesian),
    (
        "mutations::open_payload_member_cartesian_changes_chain",
        open_payload_member_cartesian_changes_chain,
    ),
    (
        "mutations::rfc8785_member_reordering_is_unchanged",
        rfc8785_member_reordering_is_unchanged,
    ),
    (
        "mutations::rfc8785_invalid_number_matrix",
        rfc8785_invalid_number_matrix,
    ),
    (
        "mutations::complete_binary_byte_flip_cartesian",
        complete_binary_byte_flip_cartesian,
    ),
    (
        "mutations::persisted_bundle_byte_flip_cartesian",
        persisted_bundle_byte_flip_cartesian,
    ),
    (
        "mutations::persisted_replay_byte_flip_cartesian",
        persisted_replay_byte_flip_cartesian,
    ),
    (
        "mutations::generation_only_changed_chain",
        generation_only_changed_chain,
    ),
    (
        "mutations::generation_only_transaction_is_unchanged",
        generation_only_transaction_is_unchanged,
    ),
    (
        "mutations::persisted_mapping_reservation_matrix",
        persisted_mapping_reservation_matrix,
    ),
    ("mutations::persisted_quota_matrix", persisted_quota_matrix),
    (
        "mutations::persisted_replay_vector_matrix",
        persisted_replay_vector_matrix,
    ),
    (
        "mutations::identical_empty_payload_swap_is_observationally_identical",
        identical_empty_payload_swap_is_observationally_identical,
    ),
    (
        "mutations::strict_replay_boundary_matrix",
        strict_replay_boundary_matrix,
    ),
    (
        "mutations::strict_replay_action_matrix",
        strict_replay_action_matrix,
    ),
    (
        "mutations::strict_replay_content_join_matrix",
        strict_replay_content_join_matrix,
    ),
    (
        "mutations::persisted_replay_tuple_matrix",
        persisted_replay_tuple_matrix,
    ),
    (
        "mutations::persisted_cross_file_join_matrix",
        persisted_cross_file_join_matrix,
    ),
    (
        "mutations::persisted_binary_framing_matrix",
        persisted_binary_framing_matrix,
    ),
    (
        "mutations::persisted_ordering_matrix",
        persisted_ordering_matrix,
    ),
    (
        "mutations::publication_expectation_begin_matrix",
        publication_expectation_begin_matrix,
    ),
    (
        "mutations::fresh_duplicate_input_matrix",
        fresh_duplicate_input_matrix,
    ),
    (
        "mutations::fresh_logical_content_changed_chain",
        fresh_logical_content_changed_chain,
    ),
    (
        "mutations::u64_max_generation_succeeds",
        u64_max_generation_succeeds,
    ),
    (
        "mutations::physical_name_bound_matrix",
        physical_name_bound_matrix,
    ),
];

const FROZEN_ROWS: &[(&str, &str, u64, &str)] = &[
    (
        "digest-rendering-cartesian",
        "mutations::rendered_digest_cartesian",
        104,
        "parse_error",
    ),
    (
        "closed-json-cartesian",
        "mutations::closed_json_cartesian",
        819,
        "parse_error",
    ),
    (
        "open-payload-fresh-derivations",
        "mutations::open_payload_member_cartesian_changes_chain",
        7,
        "changed_chain",
    ),
    (
        "rfc8785-member-reordering",
        "mutations::rfc8785_member_reordering_is_unchanged",
        7,
        "unchanged",
    ),
    (
        "rfc8785-invalid-numbers",
        "mutations::rfc8785_invalid_number_matrix",
        44,
        "parse_error",
    ),
    (
        "standalone-binary-byte-flips",
        "mutations::complete_binary_byte_flip_cartesian",
        7_594,
        "parse_error_or_changed_chain",
    ),
    (
        "persisted-class-byte-flips",
        "mutations::persisted_bundle_byte_flip_cartesian",
        16_686,
        "parse_error",
    ),
    (
        "persisted-replay-byte-flips",
        "mutations::persisted_replay_byte_flip_cartesian",
        3_075,
        "parse_error",
    ),
    (
        "fresh-generation-seven-chain",
        "mutations::generation_only_changed_chain",
        17,
        "changed_chain",
    ),
    (
        "fresh-generation-seven-transaction-control",
        "mutations::generation_only_transaction_is_unchanged",
        1,
        "unchanged",
    ),
    (
        "mapping-reservation-persisted-matrix",
        "mutations::persisted_mapping_reservation_matrix",
        18,
        "parse_error",
    ),
    (
        "quota-persisted-matrix",
        "mutations::persisted_quota_matrix",
        17,
        "parse_error",
    ),
    (
        "replay-vector-positional-matrix",
        "mutations::persisted_replay_vector_matrix",
        9,
        "parse_error",
    ),
    (
        "replay-vector-identical-empty-swap",
        "mutations::identical_empty_payload_swap_is_observationally_identical",
        1,
        "unchanged",
    ),
    (
        "strict-replay-boundary-matrix",
        "mutations::strict_replay_boundary_matrix",
        20,
        "parse_error",
    ),
    (
        "strict-replay-action-matrix",
        "mutations::strict_replay_action_matrix",
        64,
        "parse_error",
    ),
    (
        "strict-replay-content-join-matrix",
        "mutations::strict_replay_content_join_matrix",
        36,
        "parse_error",
    ),
    (
        "replay-tuple-persisted-matrix",
        "mutations::persisted_replay_tuple_matrix",
        14,
        "parse_error",
    ),
    (
        "persisted-cross-file-join-matrix",
        "mutations::persisted_cross_file_join_matrix",
        25,
        "parse_error",
    ),
    (
        "binary-framing-persisted-matrix",
        "mutations::persisted_binary_framing_matrix",
        26,
        "parse_error",
    ),
    (
        "ordering-persisted-matrix",
        "mutations::persisted_ordering_matrix",
        10,
        "parse_error",
    ),
    (
        "publication-expectation-begin-matrix",
        "mutations::publication_expectation_begin_matrix",
        23,
        "parse_error",
    ),
    (
        "fresh-input-duplicates",
        "mutations::fresh_duplicate_input_matrix",
        5,
        "parse_error",
    ),
    (
        "fresh-logical-content-changes",
        "mutations::fresh_logical_content_changed_chain",
        12,
        "changed_chain",
    ),
    (
        "compile-time-semantic-swaps",
        "ui::public_surface_and_privacy_contract",
        24,
        "compile_error",
    ),
    (
        "checked-arithmetic-private-matrix",
        "codec::tests::checked_arithmetic_matrix",
        9,
        "parse_error",
    ),
    (
        "u64-max-length-control",
        "codec::tests::checked_arithmetic_matrix",
        1,
        "success",
    ),
    (
        "u64-max-generation-control",
        "mutations::u64_max_generation_succeeds",
        1,
        "success",
    ),
    (
        "physical-name-bound",
        "mutations::physical_name_bound_matrix",
        3,
        "parse_error",
    ),
    (
        "resource-key-nul-position-cartesian",
        "resource_key_nul::embedded_nul_is_rejected_at_every_position",
        125,
        "parse_error",
    ),
];

fn matrix() -> Value {
    serde_json::from_slice(MATRIX).unwrap()
}

fn goldens() -> Value {
    serde_json::from_slice(GOLDENS).unwrap()
}

fn assert_runner(runner: &str, expected_cases: usize, outcome: &str) {
    let fixture = matrix();
    let rows = fixture["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["tier"] == "integration" && row["runner"] == runner)
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 1, "runner {runner} must have exactly one row");
    assert_eq!(rows[0]["case_count"], expected_cases as u64);
    assert_eq!(rows[0]["outcome"], outcome);
}

#[test]
fn frozen_revision_two_registry_is_exact_and_executable() {
    assert_eq!(
        format!("{:x}", Sha256::digest(MATRIX)),
        "61cad71a753fd3f614843271f29758bdcb99a61698330c2ccb63e5e195e69720",
        "the frozen revision-2 ledger bytes changed"
    );
    let fixture = matrix();
    assert_eq!(fixture["format_version"], 2);
    assert_eq!(fixture["summary"]["row_count"], 30);
    assert_eq!(fixture["summary"]["case_count"], 28_797);

    let actual = fixture["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row["id"].as_str().unwrap().to_owned(),
                row["runner"].as_str().unwrap().to_owned(),
                row["case_count"].as_u64().unwrap(),
                row["outcome"].as_str().unwrap().to_owned(),
            )
        })
        .collect::<BTreeSet<_>>();
    let frozen = FROZEN_ROWS
        .iter()
        .map(|(id, runner, count, outcome)| {
            (
                (*id).to_owned(),
                (*runner).to_owned(),
                *count,
                (*outcome).to_owned(),
            )
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(actual, frozen, "the frozen ledger row set changed");

    let registered = INTEGRATION_RUNNERS
        .iter()
        .map(|(runner, _)| *runner)
        .collect::<BTreeSet<_>>();
    let declared = fixture["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["runner"].as_str().unwrap().starts_with("mutations::"))
        .map(|row| row["runner"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(registered, declared, "integration runner registry drifted");
    assert_eq!(INTEGRATION_RUNNERS.len(), 26);
    assert_eq!(FROZEN_ROWS.len(), 30);
    assert_eq!(FROZEN_ROWS.iter().map(|row| row.2).sum::<u64>(), 28_797);
}

struct PersistedBundle {
    prepared: Vec<u8>,
    replay: Vec<Vec<u8>>,
    plan: Vec<u8>,
    begin: Vec<u8>,
}

impl PersistedBundle {
    fn from_fresh(bundle: &xerj_corpus_publication::DurableBeginBundleV1) -> Self {
        Self {
            prepared: bundle
                .prepared_input()
                .canonical_preimage()
                .canonical_preimage()
                .to_vec(),
            replay: bundle
                .replay_artifacts()
                .iter()
                .map(|artifact| artifact.bytes().artifact_bytes().to_vec())
                .collect(),
            plan: bundle
                .desired_plan()
                .canonical_preimage()
                .canonical_preimage()
                .to_vec(),
            begin: bundle
                .sync_begin()
                .canonical_json()
                .canonical_json()
                .to_vec(),
        }
    }

    fn from_oracle_fixture(fixture: &Value) -> Self {
        let classes = &fixture["bundle"]["persisted_classes"];
        Self {
            prepared: STANDARD
                .decode(classes["prepared_input"]["bytes_base64"].as_str().unwrap())
                .unwrap(),
            replay: classes["replay_artifacts"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| {
                    STANDARD
                        .decode(item["bytes_base64"].as_str().unwrap())
                        .unwrap()
                })
                .collect(),
            plan: STANDARD
                .decode(classes["desired_plan"]["bytes_base64"].as_str().unwrap())
                .unwrap(),
            begin: STANDARD
                .decode(classes["sync_begin"]["bytes_base64"].as_str().unwrap())
                .unwrap(),
        }
    }

    fn rehydrate(self) -> Result<xerj_corpus_publication::DurableBeginBundleV1, ProtocolError> {
        xerj_corpus_publication::DurableBeginBundleV1::rehydrate(
            PersistedPreparedInputBytesV1::from_journal(self.prepared.into_boxed_slice()),
            self.replay
                .into_iter()
                .map(|bytes| PersistedReplayArtifactBytesV1::from_journal(bytes.into_boxed_slice()))
                .collect(),
            PersistedDesiredPlanBytesV1::from_journal(self.plan.into_boxed_slice()),
            PersistedSyncBeginBytesV1::from_journal(self.begin.into_boxed_slice()),
        )
    }
}

fn ndjson_lines(bytes: &[u8]) -> Vec<Vec<u8>> {
    assert!(bytes.ends_with(b"\n"));
    bytes[..bytes.len() - 1]
        .split(|byte| *byte == b'\n')
        .map(<[u8]>::to_vec)
        .collect()
}

fn join_ndjson(lines: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for line in lines {
        out.extend_from_slice(line);
        out.push(b'\n');
    }
    out
}

fn reverse_ndjson_pairs(bytes: &[u8]) -> Vec<u8> {
    let lines = ndjson_lines(bytes);
    assert!(lines.len().is_multiple_of(2));
    let mut pairs = lines
        .chunks_exact(2)
        .map(<[Vec<u8>]>::to_vec)
        .collect::<Vec<_>>();
    assert!(pairs.len() >= 2);
    pairs.reverse();
    join_ndjson(&pairs.into_iter().flatten().collect::<Vec<_>>())
}

fn ordering_persisted_bundle(fixture: &Value) -> PersistedBundle {
    PersistedBundle {
        prepared: STANDARD
            .decode(
                fixture["prepared"]["prepared_input"]["preimage_base64"]
                    .as_str()
                    .unwrap(),
            )
            .unwrap(),
        replay: fixture["planned"]["artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|artifact| {
                STANDARD
                    .decode(artifact["bytes_base64"].as_str().unwrap())
                    .unwrap()
            })
            .collect(),
        plan: STANDARD
            .decode(
                fixture["planned"]["desired_plan"]["preimage_base64"]
                    .as_str()
                    .unwrap(),
            )
            .unwrap(),
        begin: STANDARD
            .decode(fixture["sync_begin"]["body_base64"].as_str().unwrap())
            .unwrap(),
    }
}

fn ordering_artifact_index(fixture: &Value, projection: &str, minimum_operations: u64) -> usize {
    fixture["planned"]["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .position(|artifact| {
            artifact["projection_kind"] == projection
                && artifact["operation_count"].as_u64().unwrap() >= minimum_operations
        })
        .unwrap()
}

fn mutate_json_line(bytes: &[u8], line: usize, change: impl FnOnce(&mut Value)) -> Vec<u8> {
    let mut lines = ndjson_lines(bytes);
    let mut value: Value = serde_json::from_slice(&lines[line]).unwrap();
    change(&mut value);
    lines[line] = serde_json::to_vec(&value).unwrap();
    join_ndjson(&lines)
}

fn duplicate_first_json_member(line: &[u8]) -> Vec<u8> {
    let value: Value = serde_json::from_slice(line).unwrap();
    let (name, member) = value.as_object().unwrap().iter().next().unwrap();
    format!(
        "{{{}:{},{}",
        serde_json::to_string(name).unwrap(),
        serde_json::to_string(member).unwrap(),
        std::str::from_utf8(&line[1..]).unwrap()
    )
    .into_bytes()
}

fn assert_rendered_cartesian<T>(baseline: &str, cross_domain: &str) -> usize
where
    T: FromStr<Err = ProtocolError>,
{
    assert!(baseline.parse::<T>().is_ok());
    let prefix_end = baseline.len() - 64;
    let cases = [
        ("wrong-prefix", format!("wrong-{}", &baseline[prefix_end..])),
        ("wrong-algorithm", baseline.replacen("sha256", "sha512", 1)),
        ("short-hex", baseline[..baseline.len() - 1].to_owned()),
        (
            "uppercase-hex",
            format!(
                "{}{}",
                &baseline[..prefix_end],
                baseline[prefix_end..].to_uppercase()
            ),
        ),
        ("leading-space", format!(" {baseline}")),
        ("trailing-space", format!("{baseline} ")),
        ("terminal-nul", format!("{baseline}\0")),
        (
            "cross-domain",
            format!("{cross_domain}{}", &baseline[prefix_end..]),
        ),
    ];
    for (case, value) in &cases {
        let error = match value.parse::<T>() {
            Ok(_) => panic!("{case} accepted for {baseline}"),
            Err(error) => error,
        };
        assert_eq!(
            error.kind(),
            ProtocolErrorKind::InvalidRenderedIdentity,
            "{case}"
        );
    }
    cases.len()
}

#[test]
fn rendered_digest_cartesian() {
    let fixture = goldens();
    let mut count = 0;
    count += assert_rendered_cartesian::<CorpusOwnerId>(
        fixture["prepared"]["owner"]["rendered"].as_str().unwrap(),
        "xercpi1-sha256-",
    );
    count += assert_rendered_cartesian::<CorpusIncarnationId>(
        fixture["prepared"]["corpus_incarnation"]["rendered"]
            .as_str()
            .unwrap(),
        "xercpo1-sha256-",
    );
    count += assert_rendered_cartesian::<ManifestDigest>(
        fixture["prepared"]["manifest"]["rendered"]
            .as_str()
            .unwrap(),
        "xermap1-sha256-",
    );
    count += assert_rendered_cartesian::<ExtractorConfigDigest>(
        fixture["prepared"]["extractor_config"]["rendered"]
            .as_str()
            .unwrap(),
        "xerm1-sha256-",
    );
    count += assert_rendered_cartesian::<MappingDigest>(
        fixture["prepared"]["mappings"]["data"]["rendered"]
            .as_str()
            .unwrap(),
        "xerecfg1-sha256-",
    );
    count += assert_rendered_cartesian::<PreparedInputDigest>(
        fixture["prepared"]["prepared_input"]["rendered"]
            .as_str()
            .unwrap(),
        "xertx1-sha256-",
    );
    count += assert_rendered_cartesian::<TransactionId>(
        fixture["generation_1"]["transaction"]["rendered"]
            .as_str()
            .unwrap(),
        "xerpdi1-sha256-",
    );
    count += assert_rendered_cartesian::<ReplayArtifactDigest>(
        fixture["generation_1"]["artifacts"][0]["digest"]["rendered"]
            .as_str()
            .unwrap(),
        "xerrs1-sha256-",
    );
    count += assert_rendered_cartesian::<ReplaySetDigest>(
        fixture["generation_1"]["replay_set"]["rendered"]
            .as_str()
            .unwrap(),
        "xerra1-sha256-",
    );
    count += assert_rendered_cartesian::<DesiredPlanDigest>(
        fixture["generation_1"]["desired_plan"]["rendered"]
            .as_str()
            .unwrap(),
        "xercp1-sha256-",
    );
    count += assert_rendered_cartesian::<PublicationDigest>(
        fixture["prior_publication"]["publication"]["rendered"]
            .as_str()
            .unwrap(),
        "xerdp1-sha256-",
    );
    count += assert_rendered_cartesian::<ExpectedPublicationDigest>(
        fixture["expectations"]["absent"]["digest"]["rendered"]
            .as_str()
            .unwrap(),
        "xersb1-sha256-",
    );
    count += assert_rendered_cartesian::<SyncBeginDigest>(
        fixture["sync_begins"]["absent"]["digest"]["rendered"]
            .as_str()
            .unwrap(),
        "xerep1-sha256-",
    );
    assert_eq!(count, 13 * 8);
    assert_runner("mutations::rendered_digest_cartesian", count, "parse_error");
}

fn enumerate_object_paths(value: &Value) -> Vec<Vec<PathSegment>> {
    fn visit(value: &Value, path: &mut Vec<PathSegment>, out: &mut Vec<Vec<PathSegment>>) {
        match value {
            Value::Object(object) => {
                out.push(path.clone());
                for (name, child) in object {
                    path.push(PathSegment::Key(name.clone()));
                    visit(child, path, out);
                    path.pop();
                }
            }
            Value::Array(items) => {
                for (index, child) in items.iter().enumerate() {
                    path.push(PathSegment::Index(index));
                    visit(child, path, out);
                    path.pop();
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    visit(value, &mut Vec::new(), &mut out);
    out
}

#[derive(Clone, Debug)]
enum PathSegment {
    Key(String),
    Index(usize),
}

fn object_mut_at<'a>(
    mut value: &'a mut Value,
    path: &[PathSegment],
) -> &'a mut serde_json::Map<String, Value> {
    for segment in path {
        value = match segment {
            PathSegment::Key(name) => value.get_mut(name).unwrap(),
            PathSegment::Index(index) => value.get_mut(*index).unwrap(),
        };
    }
    value.as_object_mut().unwrap()
}

fn duplicate_member_bytes(value: &Value, path: &[PathSegment], name: &str) -> Vec<u8> {
    let member_value = {
        let mut current = value;
        for segment in path {
            current = match segment {
                PathSegment::Key(key) => &current[key],
                PathSegment::Index(index) => &current[*index],
            };
        }
        current[name].clone()
    };
    let canonical = serde_json::to_vec(value).unwrap();
    let needle = format!(
        "{}:{}",
        serde_json::to_string(name).unwrap(),
        serde_json::to_string(&member_value).unwrap()
    );
    let text = std::str::from_utf8(&canonical).unwrap();
    text.replacen(&needle, &format!("{needle},{needle}"), 1)
        .into_bytes()
}

fn exercise_closed_object(
    baseline: &str,
    parse: impl Fn(&[u8]) -> Result<(), ProtocolError>,
) -> usize {
    let value: Value = serde_json::from_str(baseline).unwrap();
    let paths = enumerate_object_paths(&value);
    let mut count = 0;
    for (path_index, path) in paths.iter().enumerate() {
        let object = {
            let mut current = &value;
            for segment in path {
                current = match segment {
                    PathSegment::Key(name) => &current[name],
                    PathSegment::Index(index) => &current[*index],
                };
            }
            current.as_object().unwrap()
        };
        let member_names = object.keys().cloned().collect::<Vec<_>>();

        let mut unknown = value.clone();
        object_mut_at(&mut unknown, path)
            .insert(format!("unknown_mutation_{path_index}"), Value::Bool(true));
        assert!(parse(&serde_json::to_vec(&unknown).unwrap()).is_err());
        count += 1;

        for name in member_names {
            let mut missing = value.clone();
            object_mut_at(&mut missing, path).remove(&name);
            assert!(parse(&serde_json::to_vec(&missing).unwrap()).is_err());
            count += 1;

            if !object.get(&name).is_some_and(Value::is_null) {
                let mut null = value.clone();
                object_mut_at(&mut null, path).insert(name.clone(), Value::Null);
                if parse(&serde_json::to_vec(&null).unwrap()).is_err() {
                    count += 1;
                }
            }

            let duplicate = parse(&duplicate_member_bytes(&value, path, &name)).unwrap_err();
            assert_eq!(duplicate.kind(), ProtocolErrorKind::DuplicateJsonKey);
            count += 1;
        }
    }
    count
}

#[test]
fn closed_json_cartesian() {
    let fixture = goldens();
    let mut count = 0;
    count += exercise_closed_object(
        fixture["prepared"]["manifest"]["canonical_json"]
            .as_str()
            .unwrap(),
        |bytes| ManifestV1::parse_json(bytes).map(|_| ()),
    );
    for row in fixture["prepared"]["logical_edge_rows"].as_array().unwrap() {
        count += exercise_closed_object(row["canonical_json"].as_str().unwrap(), |bytes| {
            xerj_corpus_publication::LogicalEdgeRowV1::parse_json(bytes).map(|_| ())
        });
    }
    for row in fixture["prepared"]["logical_node_rows"].as_array().unwrap() {
        count += exercise_closed_object(row["canonical_json"].as_str().unwrap(), |bytes| {
            xerj_corpus_publication::LogicalNodeRowV1::parse_json(bytes).map(|_| ())
        });
    }
    count += exercise_closed_object(
        fixture["prior_publication"]["publication"]["canonical_json"]
            .as_str()
            .unwrap(),
        |bytes| CorpusPublicationV1::parse_closed_json(bytes).map(|_| ()),
    );
    for kind in ["absent", "present"] {
        count += exercise_closed_object(
            fixture["expectations"][kind]["canonical_json"]
                .as_str()
                .unwrap(),
            |bytes| ExpectedPublicationV1::parse_closed_json(bytes).map(|_| ()),
        );
        count += exercise_closed_object(
            fixture["sync_begins"][kind]["canonical_json"]
                .as_str()
                .unwrap(),
            |bytes| SyncBeginV1::parse_closed_json(bytes).map(|_| ()),
        );
    }
    assert_runner("mutations::closed_json_cartesian", count, "parse_error");
}

#[test]
fn complete_binary_byte_flip_cartesian() {
    let bundle = absent_bundle(1);
    let baselines = [
        (
            "prepared",
            bundle
                .prepared_input()
                .canonical_preimage()
                .canonical_preimage(),
        ),
        (
            "plan",
            bundle
                .desired_plan()
                .canonical_preimage()
                .canonical_preimage(),
        ),
    ];
    let mut count = 0;
    for (name, original) in baselines {
        for index in 0..original.len() {
            let mut changed = original.to_vec();
            changed[index] ^= 1;
            let changed_chain = if name == "prepared" {
                PreparedInputV1::parse_canonical_preimage(&changed)
                    .map(|parsed| parsed.digest() != bundle.prepared_input().digest())
            } else {
                DesiredPublicationPlanV1::parse_canonical_preimage(&changed)
                    .map(|parsed| parsed.digest() != bundle.desired_plan().digest())
            };
            assert!(changed_chain.is_err() || changed_chain.unwrap());
            count += 1;
        }
    }
    assert_eq!(count, 1_303 + 6_291);
    assert_runner(
        "mutations::complete_binary_byte_flip_cartesian",
        count,
        "parse_error_or_changed_chain",
    );
}

#[test]
fn rfc8785_member_reordering_is_unchanged() {
    let mut count = 0;
    assert_eq!(
        xerj_corpus_publication::DataMappingV1::parse_json(br#"{"z":2,"a":1}"#)
            .unwrap()
            .digest(),
        xerj_corpus_publication::DataMappingV1::parse_json(br#"{"a":1,"z":2}"#)
            .unwrap()
            .digest()
    );
    count += 1;
    assert_eq!(
        CatalogMappingV1::parse_json(br#"{"z":2,"a":1}"#)
            .unwrap()
            .digest(),
        CatalogMappingV1::parse_json(br#"{"a":1,"z":2}"#)
            .unwrap()
            .digest()
    );
    count += 1;
    assert_eq!(
        GraphEdgeMappingV1::parse_json(br#"{"z":2,"a":1}"#)
            .unwrap()
            .digest(),
        GraphEdgeMappingV1::parse_json(br#"{"a":1,"z":2}"#)
            .unwrap()
            .digest()
    );
    count += 1;
    assert_eq!(
        GraphNodeMappingV1::parse_json(br#"{"z":2,"a":1}"#)
            .unwrap()
            .digest(),
        GraphNodeMappingV1::parse_json(br#"{"a":1,"z":2}"#)
            .unwrap()
            .digest()
    );
    count += 1;
    assert_eq!(
        ExtractorConfigV1::parse_json(br#"{"z":2,"a":1}"#)
            .unwrap()
            .digest(),
        ExtractorConfigV1::parse_json(br#"{"a":1,"z":2}"#)
            .unwrap()
            .digest()
    );
    count += 1;
    let document_forward = absent_bundle_with(
        1,
        br#"{"z":2,"path":"alpha.md","body":"Alpha links [[beta]].","a":1}"#,
        br#"{"path":"alpha.md","kind":"file"}"#,
        br#"{}"#,
    );
    let document_reverse = absent_bundle_with(
        1,
        br#"{"a":1,"body":"Alpha links [[beta]].","path":"alpha.md","z":2}"#,
        br#"{"path":"alpha.md","kind":"file"}"#,
        br#"{}"#,
    );
    assert_eq!(
        document_forward.desired_plan().digest(),
        document_reverse.desired_plan().digest()
    );
    count += 1;
    let catalog_forward = absent_bundle_with(
        1,
        br#"{"path":"alpha.md","body":"Alpha links [[beta]]."}"#,
        br#"{"z":2,"path":"alpha.md","kind":"file","a":1}"#,
        br#"{}"#,
    );
    let catalog_reverse = absent_bundle_with(
        1,
        br#"{"path":"alpha.md","body":"Alpha links [[beta]]."}"#,
        br#"{"a":1,"kind":"file","path":"alpha.md","z":2}"#,
        br#"{}"#,
    );
    assert_eq!(
        catalog_forward.desired_plan().digest(),
        catalog_reverse.desired_plan().digest()
    );
    count += 1;
    assert_runner(
        "mutations::rfc8785_member_reordering_is_unchanged",
        count,
        "unchanged",
    );
}

#[test]
fn open_payload_member_cartesian_changes_chain() {
    let baseline = absent_bundle(1);
    let mut count = 0;
    for changed in [
        absent_bundle_with(
            1,
            br#"{"custom":{"nested":true},"path":"alpha.md","body":"Alpha links [[beta]]."}"#,
            br#"{"path":"alpha.md","kind":"file"}"#,
            br#"{}"#,
        ),
        absent_bundle_with(
            1,
            br#"{"path":"alpha.md","body":"Alpha links [[beta]]."}"#,
            br#"{"custom":{"nested":true},"path":"alpha.md","kind":"file"}"#,
            br#"{}"#,
        ),
        absent_bundle_with(
            1,
            br#"{"path":"alpha.md","body":"Alpha links [[beta]]."}"#,
            br#"{"path":"alpha.md","kind":"file"}"#,
            br#"{"custom":{"nested":true}}"#,
        ),
    ] {
        assert_ne!(
            changed.desired_plan().digest(),
            baseline.desired_plan().digest()
        );
        count += 1;
    }
    for changed in [
        xerj_corpus_publication::DataMappingV1::parse_json(br#"{"custom":1}"#)
            .unwrap()
            .digest()
            .as_rendered_str(),
        CatalogMappingV1::parse_json(br#"{"custom":1}"#)
            .unwrap()
            .digest()
            .as_rendered_str(),
        GraphEdgeMappingV1::parse_json(br#"{"custom":1}"#)
            .unwrap()
            .digest()
            .as_rendered_str(),
        GraphNodeMappingV1::parse_json(br#"{"custom":1}"#)
            .unwrap()
            .digest()
            .as_rendered_str(),
    ] {
        assert_ne!(
            changed,
            xerj_corpus_publication::DataMappingV1::parse_json(br#"{}"#)
                .unwrap()
                .digest()
                .as_rendered_str()
        );
        count += 1;
    }
    assert_eq!(count, 7);
    assert_runner(
        "mutations::open_payload_member_cartesian_changes_chain",
        count,
        "changed_chain",
    );
}

fn assert_invalid_numbers(
    family: &str,
    render: impl Fn(&str) -> Vec<u8>,
    parse: impl Fn(&[u8]) -> Result<(), ProtocolError>,
) -> usize {
    let mut count = 0;
    for spelling in ["NaN", "+Infinity", "-Infinity"] {
        let bytes = render(spelling);
        assert!(parse(&bytes).is_err(), "{family}:{spelling}");
        count += 1;
    }
    count
}

#[test]
fn rfc8785_invalid_number_matrix() {
    let fixture = goldens();
    let manifest = fixture["prepared"]["manifest"]["canonical_json"]
        .as_str()
        .unwrap();
    let edge = fixture["prepared"]["logical_edge_rows"][0]["canonical_json"]
        .as_str()
        .unwrap();
    let publication = fixture["prior_publication"]["publication"]["canonical_json"]
        .as_str()
        .unwrap();
    let expectation = fixture["expectations"]["absent"]["canonical_json"]
        .as_str()
        .unwrap();
    let begin = fixture["sync_begins"]["absent"]["canonical_json"]
        .as_str()
        .unwrap();

    let mut count = 0;
    count += assert_invalid_numbers(
        "manifest",
        |number| {
            manifest
                .replacen(":1", &format!(":{number}"), 1)
                .into_bytes()
        },
        |bytes| ManifestV1::parse_json(bytes).map(|_| ()),
    );
    count += assert_invalid_numbers(
        "data-mapping",
        |number| format!("{{\"number\":{number}}}").into_bytes(),
        |bytes| xerj_corpus_publication::DataMappingV1::parse_json(bytes).map(|_| ()),
    );
    count += assert_invalid_numbers(
        "catalog-mapping",
        |number| format!("{{\"number\":{number}}}").into_bytes(),
        |bytes| CatalogMappingV1::parse_json(bytes).map(|_| ()),
    );
    count += assert_invalid_numbers(
        "graph-edge-mapping",
        |number| format!("{{\"number\":{number}}}").into_bytes(),
        |bytes| GraphEdgeMappingV1::parse_json(bytes).map(|_| ()),
    );
    count += assert_invalid_numbers(
        "graph-node-mapping",
        |number| format!("{{\"number\":{number}}}").into_bytes(),
        |bytes| GraphNodeMappingV1::parse_json(bytes).map(|_| ()),
    );
    count += assert_invalid_numbers(
        "extractor-config",
        |number| format!("{{\"number\":{number}}}").into_bytes(),
        |bytes| ExtractorConfigV1::parse_json(bytes).map(|_| ()),
    );
    count += assert_invalid_numbers(
        "data-source",
        |number| format!("{{\"number\":{number}}}").into_bytes(),
        |bytes| {
            xerj_corpus_publication::DataDocumentV1::parse_source(
                xerj_corpus_publication::DocumentId::from_str("doc").unwrap(),
                bytes,
            )
            .map(|_| ())
        },
    );
    count += assert_invalid_numbers(
        "catalog-source",
        |number| format!("{{\"number\":{number}}}").into_bytes(),
        |bytes| {
            xerj_corpus_publication::CatalogWrapperV1::parse_public_source(
                xerj_corpus_publication::WrapperId::from_str("wrap").unwrap(),
                bytes,
            )
            .map(|_| ())
        },
    );
    count += assert_invalid_numbers(
        "logical-edge",
        |number| {
            edge.replacen("\"weight\":1", &format!("\"weight\":{number}"), 1)
                .into_bytes()
        },
        |bytes| xerj_corpus_publication::LogicalEdgeRowV1::parse_json(bytes).map(|_| ()),
    );
    count += assert_invalid_numbers(
        "logical-node",
        |number| format!("{{\"number\":{number}}}").into_bytes(),
        |bytes| xerj_corpus_publication::LogicalNodeRowV1::parse_json(bytes).map(|_| ()),
    );
    count += assert_invalid_numbers(
        "publication",
        |number| {
            publication
                .replacen("\"sequence\":1", &format!("\"sequence\":{number}"), 1)
                .into_bytes()
        },
        |bytes| CorpusPublicationV1::parse_closed_json(bytes).map(|_| ()),
    );
    count += assert_invalid_numbers(
        "expectation",
        |number| {
            expectation
                .replacen("\"sequence\":0", &format!("\"sequence\":{number}"), 1)
                .into_bytes()
        },
        |bytes| ExpectedPublicationV1::parse_closed_json(bytes).map(|_| ()),
    );
    count += assert_invalid_numbers(
        "sync-begin",
        |number| {
            begin
                .replacen(
                    "\"format_version\":1",
                    &format!("\"format_version\":{number}"),
                    1,
                )
                .into_bytes()
        },
        |bytes| SyncBeginV1::parse_closed_json(bytes).map(|_| ()),
    );

    let fractional_manifest = manifest
        .replacen("\"format_version\":1", "\"format_version\":1.5", 1)
        .into_bytes();
    assert!(
        ManifestV1::parse_json(&fractional_manifest).is_err(),
        "manifest:format_version=1.5"
    );
    count += 1;
    assert!(xerj_corpus_publication::LogicalEdgeRowV1::parse_json(
        edge.replacen("\"schema_version\":1", "\"schema_version\":1.5", 1)
            .as_bytes()
    )
    .is_err());
    count += 1;
    assert!(CorpusPublicationV1::parse_closed_json(
        publication
            .replacen("\"sequence\":1", "\"sequence\":1.5", 1)
            .as_bytes()
    )
    .is_err());
    count += 1;
    assert!(ExpectedPublicationV1::parse_closed_json(
        expectation
            .replacen("\"sequence\":0", "\"sequence\":0.5", 1)
            .as_bytes()
    )
    .is_err());
    count += 1;
    assert!(SyncBeginV1::parse_closed_json(
        begin
            .replacen("\"format_version\":1", "\"format_version\":1.5", 1)
            .as_bytes()
    )
    .is_err());
    count += 1;

    assert_runner(
        "mutations::rfc8785_invalid_number_matrix",
        count,
        "parse_error",
    );
}

#[test]
fn generation_only_changed_chain() {
    let one = absent_bundle(1);
    let seven = absent_bundle(7);
    assert_eq!(
        one.desired_plan().transaction_id(),
        seven.desired_plan().transaction_id()
    );
    assert_ne!(one.desired_plan().digest(), seven.desired_plan().digest());
    assert_ne!(one.sync_begin().digest(), seven.sync_begin().digest());
    assert_ne!(
        one.desired_plan().reserved_resource_keys(),
        seven.desired_plan().reserved_resource_keys()
    );
    assert_ne!(
        one.desired_plan()
            .mapping_reservations()
            .iter()
            .map(|item| item.resource_key().as_protocol_str())
            .collect::<Vec<_>>(),
        seven
            .desired_plan()
            .mapping_reservations()
            .iter()
            .map(|item| item.resource_key().as_protocol_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(one.replay_artifacts().len(), seven.replay_artifacts().len());
    assert!(one
        .replay_artifacts()
        .iter()
        .zip(seven.replay_artifacts())
        .all(|(left, right)| left.digest() != right.digest()));

    let cases = matrix()["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "fresh-generation-seven-chain")
        .unwrap()["cases"]
        .as_array()
        .unwrap()
        .len();
    assert_runner(
        "mutations::generation_only_changed_chain",
        cases,
        "changed_chain",
    );
}

#[test]
fn generation_only_transaction_is_unchanged() {
    let one = absent_bundle(1);
    let seven = absent_bundle(7);
    assert_eq!(
        one.desired_plan().transaction_id(),
        seven.desired_plan().transaction_id()
    );
    assert_runner(
        "mutations::generation_only_transaction_is_unchanged",
        1,
        "unchanged",
    );
}

#[test]
fn persisted_replay_vector_matrix() {
    let primary = absent_bundle(1);
    let mut mutations = Vec::new();
    for operation in 0..4 {
        let mut bytes = PersistedBundle::from_fresh(&primary);
        match operation {
            0 => {
                bytes.replay.remove(0);
            }
            1 => bytes.replay.push(Vec::new()),
            2 => bytes.replay.push(bytes.replay[0].clone()),
            3 => bytes.replay.reverse(),
            _ => unreachable!(),
        }
        mutations.push(bytes);
    }

    let fixture = goldens();
    let two_empty = &fixture["positional_fixtures"]["two_empty_same_kind"];
    let mut empty_omit = PersistedBundle::from_oracle_fixture(two_empty);
    empty_omit.replay.remove(1);
    mutations.push(empty_omit);
    let mut empty_add = PersistedBundle::from_oracle_fixture(two_empty);
    empty_add.replay.insert(1, Vec::new());
    mutations.push(empty_add);

    let two_nonempty = &fixture["positional_fixtures"]["two_nonempty_distinct"];
    let base = PersistedBundle::from_oracle_fixture(two_nonempty);
    let data_positions = two_nonempty["bundle"]["persisted_classes"]["replay_artifacts"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .filter(|(_, item)| item["projection_kind"] == "data")
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    assert_eq!(data_positions.len(), 2);
    let mut swapped = PersistedBundle::from_oracle_fixture(two_nonempty);
    swapped.replay.swap(data_positions[0], data_positions[1]);
    mutations.push(swapped);
    let mut omitted = PersistedBundle::from_oracle_fixture(two_nonempty);
    omitted.replay.remove(data_positions[0]);
    mutations.push(omitted);
    let mut added = PersistedBundle::from_oracle_fixture(two_nonempty);
    added
        .replay
        .insert(data_positions[0], base.replay[data_positions[0]].clone());
    mutations.push(added);

    for (index, mutation) in mutations.into_iter().enumerate() {
        assert!(mutation.rehydrate().is_err(), "replay vector case {index}");
    }
    assert_runner(
        "mutations::persisted_replay_vector_matrix",
        9,
        "parse_error",
    );
}

#[test]
fn identical_empty_payload_swap_is_observationally_identical() {
    let fixture = goldens();
    let two_empty = &fixture["positional_fixtures"]["two_empty_same_kind"];
    let mut persisted = PersistedBundle::from_oracle_fixture(two_empty);
    assert!(persisted.replay[1].is_empty());
    assert!(persisted.replay[2].is_empty());
    persisted.replay.swap(1, 2);
    assert!(persisted.rehydrate().is_ok());
    assert_runner(
        "mutations::identical_empty_payload_swap_is_observationally_identical",
        1,
        "unchanged",
    );
}

#[test]
fn persisted_bundle_byte_flip_cartesian() {
    let fresh = absent_bundle(1);
    let baseline = PersistedBundle::from_fresh(&fresh);
    let lengths = [
        baseline.prepared.len(),
        baseline.plan.len(),
        baseline.begin.len(),
    ];
    assert_eq!(lengths, [1_303, 6_291, 9_092]);

    let mut count = 0;
    for (class, length) in lengths.into_iter().enumerate() {
        for offset in 0..length {
            let mut persisted = PersistedBundle::from_fresh(&fresh);
            let bytes = match class {
                0 => &mut persisted.prepared,
                1 => &mut persisted.plan,
                2 => &mut persisted.begin,
                _ => unreachable!(),
            };
            bytes[offset] ^= 1;
            assert!(
                persisted.rehydrate().is_err(),
                "class {class} offset {offset}"
            );
            count += 1;
        }
    }
    assert_runner(
        "mutations::persisted_bundle_byte_flip_cartesian",
        count,
        "parse_error",
    );
}

#[test]
fn persisted_replay_byte_flip_cartesian() {
    let fresh = absent_bundle(1);
    let lengths = PersistedBundle::from_fresh(&fresh)
        .replay
        .iter()
        .map(Vec::len)
        .collect::<Vec<_>>();
    assert_eq!(lengths, [204, 585, 943, 1_343]);

    let mut count = 0;
    for (artifact, length) in lengths.into_iter().enumerate() {
        for offset in 0..length {
            let mut persisted = PersistedBundle::from_fresh(&fresh);
            persisted.replay[artifact][offset] ^= 1;
            assert!(
                persisted.rehydrate().is_err(),
                "artifact {artifact} offset {offset}"
            );
            count += 1;
        }
    }
    assert_runner(
        "mutations::persisted_replay_byte_flip_cartesian",
        count,
        "parse_error",
    );
}

#[test]
fn persisted_mapping_reservation_matrix() {
    let bundle = absent_bundle(1);
    let plan = bundle
        .desired_plan()
        .canonical_preimage()
        .canonical_preimage();
    for case in 0..18 {
        let mutated = mutation_plan::mapping_case(plan, case);
        assert!(
            DesiredPublicationPlanV1::parse_canonical_preimage(&mutated).is_err(),
            "mapping reservation case {case}"
        );
    }
    assert_runner(
        "mutations::persisted_mapping_reservation_matrix",
        18,
        "parse_error",
    );
}

#[test]
fn persisted_quota_matrix() {
    let bundle = absent_bundle(1);
    let plan = bundle
        .desired_plan()
        .canonical_preimage()
        .canonical_preimage();
    let cases = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 14, 15, 17, 18];
    for case in cases {
        let mutated = mutation_plan::quota_case(plan, case);
        let error = DesiredPublicationPlanV1::parse_canonical_preimage(&mutated).unwrap_err();
        match case {
            13 => {
                assert_eq!(error.kind(), ProtocolErrorKind::ArithmeticOverflow);
                assert!(error
                    .to_string()
                    .contains("artifact charge addition overflow"));
            }
            14 => {
                assert_eq!(error.kind(), ProtocolErrorKind::ArithmeticOverflow);
                assert!(error
                    .to_string()
                    .contains("operation count addition overflow"));
            }
            15 => {
                assert_eq!(error.kind(), ProtocolErrorKind::ArithmeticOverflow);
                assert!(error
                    .to_string()
                    .contains("operation charge multiplication overflow"));
            }
            17 | 18 => {
                assert_eq!(error.kind(), ProtocolErrorKind::ArithmeticOverflow);
                assert!(error.to_string().contains("stage charge addition overflow"));
            }
            _ => assert_ne!(error.kind(), ProtocolErrorKind::ArithmeticOverflow),
        }
    }
    assert_runner("mutations::persisted_quota_matrix", 17, "parse_error");
}

#[test]
fn persisted_replay_tuple_matrix() {
    let bundle = absent_bundle(1);
    let plan = bundle
        .desired_plan()
        .canonical_preimage()
        .canonical_preimage();
    for case in 0..14 {
        let mutated = mutation_plan::tuple_case(plan, case);
        assert!(
            DesiredPublicationPlanV1::parse_canonical_preimage(&mutated).is_err(),
            "replay tuple case {case}"
        );
    }
    assert_runner(
        "mutations::persisted_replay_tuple_matrix",
        14,
        "parse_error",
    );
}

#[test]
fn strict_replay_boundary_matrix() {
    let fresh = absent_bundle(1);
    let mut cases = Vec::new();
    let data = &PersistedBundle::from_fresh(&fresh).replay[1];
    let data_lines = ndjson_lines(data);

    let mut odd = data_lines.clone();
    odd.pop();
    cases.push((1, join_ndjson(&odd)));
    let mut blank_action = data_lines.clone();
    blank_action[0].clear();
    cases.push((1, join_ndjson(&blank_action)));
    let mut blank_source = data_lines.clone();
    blank_source[1].clear();
    cases.push((1, join_ndjson(&blank_source)));
    let mut missing_source = data_lines.clone();
    missing_source.remove(1);
    cases.push((1, join_ndjson(&missing_source)));
    let mut extra_source = data_lines.clone();
    extra_source.push(b"{}".to_vec());
    cases.push((1, join_ndjson(&extra_source)));
    cases.push((1, data[..data.len() - 1].to_vec()));
    let mut extra_lf = data.clone();
    extra_lf.push(b'\n');
    cases.push((1, extra_lf));
    let first_lf = data.iter().position(|byte| *byte == b'\n').unwrap();
    let mut crlf = data.clone();
    crlf.insert(first_lf, b'\r');
    cases.push((1, crlf));
    let mut embedded_cr = data.clone();
    embedded_cr.insert(1, b'\r');
    cases.push((1, embedded_cr));
    let mut trailing_space = data.clone();
    trailing_space.push(b' ');
    cases.push((1, trailing_space));
    let mut noncanonical_action = data_lines.clone();
    noncanonical_action[0].insert(1, b' ');
    cases.push((1, join_ndjson(&noncanonical_action)));
    let mut noncanonical_source = data_lines.clone();
    noncanonical_source[1].insert(1, b' ');
    cases.push((1, join_ndjson(&noncanonical_source)));
    let mut duplicate_action = data_lines.clone();
    duplicate_action[0] = duplicate_first_json_member(&duplicate_action[0]);
    cases.push((1, join_ndjson(&duplicate_action)));
    let mut duplicate_source = data_lines.clone();
    duplicate_source[1] = duplicate_first_json_member(&duplicate_source[1]);
    cases.push((1, join_ndjson(&duplicate_source)));

    for (artifact, operation, line) in [
        (0usize, "missing-lf", 0usize),
        (0, "duplicate-action", 0),
        (2, "missing-lf", 0),
        (2, "duplicate-source", 1),
        (3, "missing-lf", 0),
        (3, "noncanonical-source", 1),
    ] {
        let baseline = &PersistedBundle::from_fresh(&fresh).replay[artifact];
        let changed = match operation {
            "missing-lf" => baseline[..baseline.len() - 1].to_vec(),
            "duplicate-action" | "duplicate-source" => {
                let mut lines = ndjson_lines(baseline);
                lines[line] = duplicate_first_json_member(&lines[line]);
                join_ndjson(&lines)
            }
            "noncanonical-source" => {
                let mut lines = ndjson_lines(baseline);
                lines[line].insert(1, b' ');
                join_ndjson(&lines)
            }
            _ => unreachable!(),
        };
        cases.push((artifact, changed));
    }
    assert_eq!(cases.len(), 20);
    for (case, (artifact, bytes)) in cases.into_iter().enumerate() {
        let mut persisted = PersistedBundle::from_fresh(&fresh);
        persisted.replay[artifact] = bytes;
        assert!(
            persisted.rehydrate().is_err(),
            "replay boundary case {case}"
        );
    }
    assert_runner(
        "mutations::strict_replay_boundary_matrix",
        20,
        "parse_error",
    );
}

fn common_action_mutation(bytes: &[u8], case: usize) -> Vec<u8> {
    mutate_json_line(bytes, 0, |value| {
        let root = value.as_object_mut().unwrap();
        match case {
            0..=2 => {
                let metadata = root.remove("index").unwrap();
                root.insert(["create", "update", "delete"][case].to_owned(), metadata);
            }
            3 => {
                root.remove("index");
            }
            4 => {
                root.insert("index".to_owned(), Value::Null);
            }
            5 => {
                root.insert("unknown".to_owned(), Value::Bool(true));
            }
            6 => {
                root["index"].as_object_mut().unwrap().remove("_id");
            }
            7 => root["index"]["_id"] = Value::Null,
            8 => root["index"]["_id"] = Value::from(7),
            9 => {
                root["index"].as_object_mut().unwrap().remove("_index");
            }
            10 => root["index"]["_index"] = Value::Null,
            11 => root["index"]["_index"] = Value::from(7),
            12 => {
                root["index"]
                    .as_object_mut()
                    .unwrap()
                    .insert("unknown".to_owned(), Value::Bool(true));
            }
            13 => root["index"]["_index"] = Value::String("wrong-target".to_owned()),
            _ => unreachable!(),
        }
    })
}

#[test]
fn strict_replay_action_matrix() {
    let fresh = absent_bundle(1);
    let baseline = PersistedBundle::from_fresh(&fresh);
    let mut cases = Vec::new();
    for artifact in 0..4 {
        for case in 0..14 {
            cases.push((
                artifact,
                common_action_mutation(&baseline.replay[artifact], case),
            ));
        }
    }
    for case in 0..4 {
        let changed = mutate_json_line(&baseline.replay[0], 0, |value| {
            let metadata = &mut value["index"];
            match case {
                0 => {
                    metadata.as_object_mut().unwrap().remove("generation");
                }
                1 => metadata["generation"] = Value::Null,
                2 => metadata["generation"] = Value::String("wrong".to_owned()),
                3 => {
                    metadata
                        .as_object_mut()
                        .unwrap()
                        .insert("generation_sibling".to_owned(), Value::from(1));
                }
                _ => unreachable!(),
            }
        });
        cases.push((0, changed));
    }
    for artifact in 1..4 {
        cases.push((
            artifact,
            mutate_json_line(&baseline.replay[artifact], 0, |value| {
                value["index"]["generation"] = Value::String("forbidden".to_owned());
            }),
        ));
    }
    let mixed = mutate_json_line(&baseline.replay[1], 2, |value| {
        value["index"]["_index"] = Value::String("mixed-target".to_owned());
    });
    cases.push((1, mixed));

    assert_eq!(cases.len(), 64);
    for (case, (artifact, bytes)) in cases.into_iter().enumerate() {
        let mut persisted = PersistedBundle::from_fresh(&fresh);
        persisted.replay[artifact] = bytes;
        assert!(persisted.rehydrate().is_err(), "replay action case {case}");
    }
    assert_runner("mutations::strict_replay_action_matrix", 64, "parse_error");
}

fn read_u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_be_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

fn skip_binary_string(bytes: &[u8], offset: &mut usize) {
    let len = usize::try_from(read_u64_at(bytes, *offset)).unwrap();
    *offset += 8 + len;
}

fn data_count_offset(bytes: &[u8], plan: bool, domain_len: usize) -> usize {
    let mut offset = domain_len + 4;
    if plan {
        for _ in 0..4 {
            skip_binary_string(bytes, &mut offset);
        }
        offset += 16;
        for _ in 0..4 {
            skip_binary_string(bytes, &mut offset);
        }
        offset += 8;
        skip_binary_string(bytes, &mut offset);
    } else {
        for _ in 0..3 {
            skip_binary_string(bytes, &mut offset);
        }
    }
    offset
}

fn binary_framing_cases(bytes: &[u8], domain_len: usize, plan: bool) -> Vec<Vec<u8>> {
    let version = domain_len;
    let first_u64 = domain_len + 4;
    let count = data_count_offset(bytes, plan, domain_len);
    let first_string_end = first_u64 + 8 + usize::try_from(read_u64_at(bytes, first_u64)).unwrap();
    let second_string_end =
        first_string_end + 8 + usize::try_from(read_u64_at(bytes, first_string_end)).unwrap();

    let mut cases = Vec::new();
    let mut value = bytes.to_vec();
    value.remove(domain_len - 1);
    cases.push(value);
    let mut value = bytes.to_vec();
    value.swap(domain_len - 2, domain_len - 1);
    cases.push(value);
    let mut value = bytes.to_vec();
    value[version..version + 4].copy_from_slice(&2u32.to_be_bytes());
    cases.push(value);
    let mut value = bytes.to_vec();
    value[version..version + 4].reverse();
    cases.push(value);
    let mut value = bytes.to_vec();
    value[first_u64..first_u64 + 8].reverse();
    cases.push(value);
    let mut value = bytes.to_vec();
    value.remove(version);
    cases.push(value);
    let mut value = bytes.to_vec();
    value.remove(first_u64);
    cases.push(value);
    let mut value = bytes.to_vec();
    value.drain(first_u64..first_string_end);
    cases.push(value);
    let mut value = bytes.to_vec();
    let first = value[first_u64..first_string_end].to_vec();
    let second = value[first_string_end..second_string_end].to_vec();
    value.splice(
        first_u64..second_string_end,
        second.into_iter().chain(first),
    );
    cases.push(value);
    let mut value = bytes.to_vec();
    let original = read_u64_at(&value, count);
    value[count..count + 8].copy_from_slice(&original.saturating_sub(1).to_be_bytes());
    cases.push(value);
    let mut value = bytes.to_vec();
    value[count..count + 8].copy_from_slice(&(read_u64_at(bytes, count) + 1).to_be_bytes());
    cases.push(value);
    let mut value = bytes.to_vec();
    value.splice(count..count, bytes[count..count + 8].iter().copied());
    cases.push(value);
    let mut value = bytes.to_vec();
    value.push(0);
    cases.push(value);
    cases
}

#[test]
fn persisted_binary_framing_matrix() {
    let bundle = absent_bundle(1);
    let prepared = bundle
        .prepared_input()
        .canonical_preimage()
        .canonical_preimage();
    let plan = bundle
        .desired_plan()
        .canonical_preimage()
        .canonical_preimage();
    let cases = [
        binary_framing_cases(prepared, b"xerj-prepared-input-v1\0".len(), false),
        binary_framing_cases(plan, b"xerj-desired-publication-plan-v1\0".len(), true),
    ];
    let mut count = 0;
    for (class, mutations) in cases.into_iter().enumerate() {
        for (case, bytes) in mutations.into_iter().enumerate() {
            let rejected = if class == 0 {
                PreparedInputV1::parse_canonical_preimage(&bytes).is_err()
            } else {
                DesiredPublicationPlanV1::parse_canonical_preimage(&bytes).is_err()
            };
            assert!(rejected, "binary framing class {class} case {case}");
            count += 1;
        }
    }
    assert_runner(
        "mutations::persisted_binary_framing_matrix",
        count,
        "parse_error",
    );
}

fn flip_after(bytes: &mut [u8], needle: &[u8]) {
    let start = bytes
        .windows(needle.len())
        .position(|window| window == needle)
        .unwrap_or_else(|| {
            panic!(
                "missing mutation needle {:?}",
                String::from_utf8_lossy(needle)
            )
        });
    let index = start + needle.len();
    bytes[index] = if bytes[index] == b'0' { b'1' } else { b'0' };
}

fn mutate_source_field(bytes: &[u8], source_line: usize, field: &str, value: Value) -> Vec<u8> {
    mutate_json_line(bytes, source_line, |source| {
        source
            .as_object_mut()
            .unwrap()
            .insert(field.to_owned(), value);
    })
}

#[test]
fn strict_replay_content_join_matrix() {
    let fresh = absent_bundle(1);
    let baseline = PersistedBundle::from_fresh(&fresh);
    let mut cases = Vec::new();

    let mut duplicate_data = ndjson_lines(&baseline.replay[1]);
    duplicate_data.extend_from_within(0..2);
    let mut case = PersistedBundle::from_fresh(&fresh);
    case.replay[1] = join_ndjson(&duplicate_data);
    cases.push(case);
    let mut reverse_data = ndjson_lines(&baseline.replay[1]);
    reverse_data.rotate_left(2);
    let mut case = PersistedBundle::from_fresh(&fresh);
    case.replay[1] = join_ndjson(&reverse_data);
    cases.push(case);
    for changed in [
        mutate_source_field(
            &baseline.replay[1],
            1,
            "body",
            Value::String("changed".to_owned()),
        ),
        mutate_source_field(
            &baseline.replay[1],
            1,
            "path",
            Value::String("changed.md".to_owned()),
        ),
        mutate_json_line(&baseline.replay[1], 0, |action| {
            action["index"]["_id"] = Value::String("changed-id".to_owned());
        }),
    ] {
        let mut case = PersistedBundle::from_fresh(&fresh);
        case.replay[1] = changed;
        cases.push(case);
    }
    let mut case = PersistedBundle::from_fresh(&fresh);
    flip_after(&mut case.prepared, b"xerra1-sha256-");
    cases.push(case);

    let catalog_lines = ndjson_lines(&baseline.replay[0]);
    let mut duplicate_catalog = catalog_lines.clone();
    duplicate_catalog.extend_from_within(0..2);
    let mut case = PersistedBundle::from_fresh(&fresh);
    case.replay[0] = join_ndjson(&duplicate_catalog);
    cases.push(case);
    let fixture = goldens();
    let ordering = &fixture["ordering_matrix"];
    let catalog_index = ordering_artifact_index(ordering, "catalog", 2);
    let mut case = ordering_persisted_bundle(ordering);
    case.replay[catalog_index] = reverse_ndjson_pairs(&case.replay[catalog_index]);
    cases.push(case);
    for changed in [
        mutate_source_field(
            &baseline.replay[0],
            1,
            "path",
            Value::String("changed.md".to_owned()),
        ),
        mutate_json_line(&baseline.replay[0], 0, |action| {
            action["index"]["_id"] = Value::String("changed-wrapper".to_owned());
        }),
    ] {
        let mut case = PersistedBundle::from_fresh(&fresh);
        case.replay[0] = changed;
        cases.push(case);
    }
    let mut case = PersistedBundle::from_fresh(&fresh);
    let second_artifact = case
        .prepared
        .windows(b"xerra1-sha256-".len())
        .enumerate()
        .filter(|(_, window)| *window == b"xerra1-sha256-")
        .nth(1)
        .map(|(index, _)| index)
        .unwrap();
    case.prepared[second_artifact + b"xerra1-sha256-".len()] ^= 1;
    cases.push(case);

    let edge_lines = ndjson_lines(&baseline.replay[2]);
    let mut duplicate_edge = edge_lines.clone();
    duplicate_edge.extend_from_within(0..2);
    let mut case = PersistedBundle::from_fresh(&fresh);
    case.replay[2] = join_ndjson(&duplicate_edge);
    cases.push(case);
    let edge_index = ordering_artifact_index(ordering, "graph-edge", 2);
    let mut case = ordering_persisted_bundle(ordering);
    case.replay[edge_index] = reverse_ndjson_pairs(&case.replay[edge_index]);
    cases.push(case);
    for (field, value) in [
        (
            "logical_edge_id",
            Value::String("00000000000000000000000000000000".to_owned()),
        ),
        (
            "physical_id",
            Value::String(format!("xerge1-sha256-{}", "0".repeat(64))),
        ),
        (
            "graph_owner",
            Value::String(format!("xercpo1-sha256-{}", "0".repeat(64))),
        ),
        (
            "corpus_incarnation",
            Value::String(format!("xercpi1-sha256-{}", "0".repeat(64))),
        ),
        (
            "tx_id",
            Value::String(format!("xertx1-sha256-{}", "0".repeat(64))),
        ),
        ("graph_generation", Value::from(7)),
        (
            "graph_producer",
            Value::String(format!("xerp1-sha256-{}", "0".repeat(64))),
        ),
        ("edge_scope", Value::String("wrong".to_owned())),
    ] {
        let mut case = PersistedBundle::from_fresh(&fresh);
        case.replay[2] = mutate_source_field(&baseline.replay[2], 1, field, value);
        cases.push(case);
    }

    let node_lines = ndjson_lines(&baseline.replay[3]);
    let mut duplicate_node = node_lines.clone();
    duplicate_node.extend_from_within(0..2);
    let mut case = PersistedBundle::from_fresh(&fresh);
    case.replay[3] = join_ndjson(&duplicate_node);
    cases.push(case);
    let mut reversed_node = node_lines.clone();
    reversed_node.rotate_left(2);
    let mut case = PersistedBundle::from_fresh(&fresh);
    case.replay[3] = join_ndjson(&reversed_node);
    cases.push(case);
    for (field, value) in [
        (
            "physical_id",
            Value::String(format!("xergn1-sha256-{}", "0".repeat(64))),
        ),
        (
            "graph_owner",
            Value::String(format!("xercpo1-sha256-{}", "0".repeat(64))),
        ),
        (
            "corpus_incarnation",
            Value::String(format!("xercpi1-sha256-{}", "0".repeat(64))),
        ),
        (
            "tx_id",
            Value::String(format!("xertx1-sha256-{}", "0".repeat(64))),
        ),
        ("graph_generation", Value::from(7)),
        ("doc_kind", Value::String("wrong".to_owned())),
    ] {
        let mut case = PersistedBundle::from_fresh(&fresh);
        case.replay[3] = mutate_source_field(&baseline.replay[3], 1, field, value);
        cases.push(case);
    }

    for prefix in [
        b"xergle1-sha256-".as_slice(),
        b"xergln1-sha256-".as_slice(),
        b"xergepi1-sha256-".as_slice(),
        b"xergnpi1-sha256-".as_slice(),
        b"xergpc1-sha256-".as_slice(),
        b"xergt1-sha256-".as_slice(),
        b"xergp1-sha256-".as_slice(),
    ] {
        let mut case = PersistedBundle::from_fresh(&fresh);
        flip_after(&mut case.plan, prefix);
        cases.push(case);
    }

    assert_eq!(cases.len(), 36);
    for (index, case) in cases.into_iter().enumerate() {
        assert!(case.rehydrate().is_err(), "content join case {index}");
    }
    assert_runner(
        "mutations::strict_replay_content_join_matrix",
        36,
        "parse_error",
    );
}

fn mutate_begin_json(bytes: &[u8], change: impl FnOnce(&mut Value)) -> Vec<u8> {
    let mut value: Value = serde_json::from_slice(bytes).unwrap();
    change(&mut value);
    serde_json::to_vec(&value).unwrap()
}

#[test]
fn persisted_cross_file_join_matrix() {
    let fresh = absent_bundle(1);
    let seven = absent_bundle(7);
    let two = absent_bundle(2);
    let mut cases: Vec<(&str, PersistedBundle)> = Vec::new();

    let mut case = PersistedBundle::from_fresh(&fresh);
    case.plan = seven
        .desired_plan()
        .canonical_preimage()
        .canonical_preimage()
        .to_vec();
    cases.push((
        "standalone-plan-bytes-from-generation-7-with-generation-1-begin",
        case,
    ));
    let mut case = PersistedBundle::from_fresh(&fresh);
    case.plan = two
        .desired_plan()
        .canonical_preimage()
        .canonical_preimage()
        .to_vec();
    cases.push(("standalone-plan-byte-change-with-begin-held", case));
    let mut case = PersistedBundle::from_fresh(&fresh);
    case.begin = two.sync_begin().canonical_json().canonical_json().to_vec();
    cases.push(("embedded-plan-byte-change-with-standalone-held", case));

    for (name, class, prefix) in [
        (
            "prepared-digest-mismatch-plan",
            "plan",
            b"xerpdi1-sha256-".as_slice(),
        ),
        (
            "prepared-digest-mismatch-begin",
            "begin",
            b"xerpdi1-sha256-".as_slice(),
        ),
        (
            "replay-set-digest-mismatch-plan",
            "plan",
            b"xerrs1-sha256-".as_slice(),
        ),
        (
            "replay-set-digest-mismatch-begin",
            "begin",
            b"xerrs1-sha256-".as_slice(),
        ),
        (
            "plan-digest-mismatch-begin",
            "begin",
            b"xerdp1-sha256-".as_slice(),
        ),
    ] {
        let mut case = PersistedBundle::from_fresh(&fresh);
        flip_after(
            if class == "plan" {
                &mut case.plan
            } else {
                &mut case.begin
            },
            prefix,
        );
        cases.push((name, case));
    }

    for (name, field_case) in [
        ("owner-mismatch-prepared-plan", 0),
        ("incarnation-mismatch-prepared-plan", 1),
        ("manifest-mismatch-prepared-plan", 2),
        ("data-count-mismatch-prepared-plan-replay", 3),
        ("catalog-count-mismatch-prepared-plan-replay", 4),
        ("graph-edge-count-mismatch-prepared-plan-replay", 5),
        ("graph-node-count-mismatch-prepared-plan-replay", 6),
    ] {
        let mut case = PersistedBundle::from_fresh(&fresh);
        case.plan = mutation_plan::cross_file_plan_field_case(&case.plan, field_case);
        cases.push((name, case));
    }

    for (name, prefix) in [
        (
            "data-id-digest-mismatch-prepared-plan-replay",
            b"xerids1-sha256-".as_slice(),
        ),
        (
            "data-content-digest-mismatch-prepared-plan-replay",
            b"xerdc1-sha256-".as_slice(),
        ),
        (
            "catalog-id-digest-mismatch-prepared-plan-replay",
            b"xercids1-sha256-".as_slice(),
        ),
        (
            "catalog-content-digest-mismatch-prepared-plan-replay",
            b"xercws1-sha256-".as_slice(),
        ),
        (
            "graph-core-mismatch-prepared-plan-replay",
            b"xergpc1-sha256-".as_slice(),
        ),
    ] {
        let mut case = PersistedBundle::from_fresh(&fresh);
        flip_after(&mut case.plan, prefix);
        cases.push((name, case));
    }

    for (name, field, value) in [
        (
            "begin-expected-owner-mismatch-plan-owner",
            "owner",
            Value::String(format!("xercpo1-sha256-{}", "0".repeat(64))),
        ),
        (
            "begin-publication-sequence-changed-with-seals-and-digest-held",
            "sequence",
            Value::from(2),
        ),
        (
            "begin-expected-root-mismatch-plan-root",
            "root_identity",
            Value::String("/wrong".to_owned()),
        ),
        (
            "begin-expected-prefix-mismatch-plan-prefix",
            "prefix",
            Value::String("wrong".to_owned()),
        ),
        (
            "begin-expected-incarnation-mismatch-plan-incarnation",
            "incarnation",
            Value::String(format!("xercpi1-sha256-{}", "0".repeat(64))),
        ),
    ] {
        let present = present_bundle();
        let mut case = PersistedBundle::from_fresh(&present);
        case.begin = mutate_begin_json(&case.begin, |begin| {
            begin["expected_publication"]["publication"]
                .as_object_mut()
                .unwrap()
                .insert(field.to_owned(), value);
        });
        cases.push((name, case));
    }

    assert_eq!(cases.len(), 25);
    for (name, case) in cases {
        let error = case.rehydrate().unwrap_err();
        assert!(
            matches!(
                error.kind(),
                ProtocolErrorKind::CrossFieldMismatch | ProtocolErrorKind::InvalidRenderedIdentity
            ),
            "{name}: {error:?}"
        );
    }
    assert_runner(
        "mutations::persisted_cross_file_join_matrix",
        25,
        "parse_error",
    );
}

#[test]
fn persisted_ordering_matrix() {
    let fixture = goldens();
    let ordering = &fixture["ordering_matrix"];
    let publication = fixture["ordering_matrix"]["prior_publication"]["publication"]
        ["canonical_json"]
        .as_str()
        .unwrap();
    let mut count = 0;

    let mut case = ordering_persisted_bundle(ordering);
    case.prepared = mutation_plan::reverse_prepared_data_routes(&case.prepared);
    let error = case.rehydrate().unwrap_err();
    assert_eq!(error.kind(), ProtocolErrorKind::NonCanonicalEncoding);
    assert!(error.to_string().contains("prepared data routes"));
    count += 1;

    for (name, projection) in [
        ("data-rows", "data"),
        ("catalog-rows", "catalog"),
        ("logical-edges", "graph-edge"),
        ("logical-nodes", "graph-node"),
    ] {
        let artifact = ordering_artifact_index(ordering, projection, 2);
        let mut case = ordering_persisted_bundle(ordering);
        case.replay[artifact] = reverse_ndjson_pairs(&case.replay[artifact]);
        let error = case.rehydrate().unwrap_err();
        assert_eq!(
            error.kind(),
            ProtocolErrorKind::NonCanonicalEncoding,
            "{name}"
        );
        assert!(
            error.to_string().contains("normative logical order"),
            "{name}: {error}"
        );
        count += 1;
    }

    let mut case = ordering_persisted_bundle(ordering);
    case.plan = mutation_plan::reverse_plan_data_entries(&case.plan);
    let error = case.rehydrate().unwrap_err();
    assert_eq!(error.kind(), ProtocolErrorKind::NonCanonicalEncoding);
    assert!(error.to_string().contains("data entries"));
    count += 1;

    for (name, case_index, reason) in [
        ("mapping-reservations", 0, "mapping reservations"),
        ("reserved-resource-keys", 1, "resource keys"),
        ("replay-tuples", 2, "replay tuples"),
    ] {
        let mut case = ordering_persisted_bundle(ordering);
        case.plan = mutation_plan::ordering_case(&case.plan, case_index);
        let error = case.rehydrate().unwrap_err();
        assert_eq!(
            error.kind(),
            ProtocolErrorKind::NonCanonicalEncoding,
            "{name}"
        );
        assert!(error.to_string().contains(reason), "{name}: {error}");
        count += 1;
    }

    let mut publication_value: Value = serde_json::from_str(publication).unwrap();
    publication_value["data"]["indices"]
        .as_array_mut()
        .unwrap()
        .reverse();
    assert!(CorpusPublicationV1::parse_closed_json(
        &serde_json::to_vec(&publication_value).unwrap()
    )
    .is_err());
    count += 1;

    assert_eq!(count, 10);
    assert_runner("mutations::persisted_ordering_matrix", count, "parse_error");
}

#[test]
fn publication_expectation_begin_matrix() {
    let fixture = goldens();
    let publication = fixture["prior_publication"]["publication"]["canonical_json"]
        .as_str()
        .unwrap();
    let mut publication_cases = Vec::new();
    for change in 0..11 {
        let mut value: Value = serde_json::from_str(publication).unwrap();
        match change {
            0 => {
                value["data"]["indices"][0]["physical_index_incarnation"] =
                    Value::String(format!("xersi1-sha256-{}", "0".repeat(64)))
            }
            1 => {
                value["data"]["projection_digest"] =
                    Value::String(format!("xerd1-sha256-{}", "0".repeat(64)))
            }
            2 => {
                value["catalog"]["projection_digest"] =
                    Value::String(format!("xercatp1-sha256-{}", "0".repeat(64)))
            }
            3 => {
                value["graph"]["projection_digest"] =
                    Value::String(format!("xergp1-sha256-{}", "0".repeat(64)))
            }
            4 => {
                value["data"]["indices"][0]["seal"]["seal_digest"] =
                    Value::String(format!("xerds1-sha256-{}", "0".repeat(64)))
            }
            5 => {
                value["catalog"]["seal"]["seal_digest"] =
                    Value::String(format!("xercs1-sha256-{}", "0".repeat(64)))
            }
            6 => {
                value["graph"]["edge_seal"]["seal_digest"] =
                    Value::String(format!("xerges1-sha256-{}", "0".repeat(64)))
            }
            7 => {
                value["graph"]["node_seal"]["seal_digest"] =
                    Value::String(format!("xergns1-sha256-{}", "0".repeat(64)))
            }
            8 => value["data"]["indices"][0]["document_count"] = Value::from(99),
            9 => value["data"]["indices"][0]["seal"]["final_write_sequence"] = Value::from(99),
            10 => {
                value["publication_digest"] =
                    Value::String(format!("xercp1-sha256-{}", "0".repeat(64)))
            }
            _ => unreachable!(),
        }
        publication_cases.push(serde_json::to_vec(&value).unwrap());
    }
    for (case, bytes) in publication_cases.into_iter().enumerate() {
        assert!(
            CorpusPublicationV1::parse_closed_json(&bytes).is_err(),
            "publication case {case}"
        );
    }

    let absent = fixture["expectations"]["absent"]["canonical_json"]
        .as_str()
        .unwrap();
    let mut absent_value: Value = serde_json::from_str(absent).unwrap();
    absent_value["sequence"] = Value::from(1);
    assert!(
        ExpectedPublicationV1::parse_closed_json(&serde_json::to_vec(&absent_value).unwrap())
            .is_err()
    );

    let present = fixture["expectations"]["present"]["canonical_json"]
        .as_str()
        .unwrap();
    for change in 0..5 {
        let mut value: Value = serde_json::from_str(present).unwrap();
        match change {
            0 => value["publication"]["sequence"] = Value::from(99),
            1 => {
                value["publication"]["publication_digest"] =
                    Value::String(format!("xercp1-sha256-{}", "0".repeat(64)))
            }
            2 => {
                value["publication"]["owner"] =
                    Value::String(format!("xercpo1-sha256-{}", "0".repeat(64)))
            }
            3 => {
                value["publication"]["incarnation"] =
                    Value::String(format!("xercpi1-sha256-{}", "0".repeat(64)))
            }
            4 => {
                value["publication"]["graph"]["node_seal"]["seal_digest"] =
                    Value::String(format!("xergns1-sha256-{}", "0".repeat(64)))
            }
            _ => unreachable!(),
        }
        assert!(
            ExpectedPublicationV1::parse_closed_json(&serde_json::to_vec(&value).unwrap()).is_err()
        );
    }

    let begin = fixture["sync_begins"]["absent"]["canonical_json"]
        .as_str()
        .unwrap();
    for change in 0..6 {
        let bytes = mutate_begin_json(begin.as_bytes(), |value| match change {
            0 => {
                value["canonical_plan_bytes"] = Value::String(format!(
                    "{}=",
                    value["canonical_plan_bytes"].as_str().unwrap()
                ));
            }
            1 => {
                let encoded = value["canonical_plan_bytes"].as_str().unwrap();
                value["canonical_plan_bytes"] =
                    Value::String(encoded[..encoded.len() - 4].to_owned());
            }
            2 => value["plan_digest"] = Value::String(format!("xerdp1-sha256-{}", "0".repeat(64))),
            3 => {
                value["prepared_input_digest"] =
                    Value::String(format!("xerpdi1-sha256-{}", "0".repeat(64)))
            }
            4 => {
                value["replay_set_digest"] =
                    Value::String(format!("xerrs1-sha256-{}", "0".repeat(64)))
            }
            5 => {
                value["sync_begin_digest"] =
                    Value::String(format!("xersb1-sha256-{}", "0".repeat(64)))
            }
            _ => unreachable!(),
        });
        assert!(
            SyncBeginV1::parse_closed_json(&bytes).is_err(),
            "begin case {change}"
        );
    }
    assert_runner(
        "mutations::publication_expectation_begin_matrix",
        23,
        "parse_error",
    );
}

#[test]
fn fresh_duplicate_input_matrix() {
    use xerj_corpus_publication::{
        BrainName, CatalogInputV1, CatalogWrapperV1, CorpusIncarnationSeed, CorpusPrefix,
        DataDocumentV1, DataMappingV1, DataRouteInputV1, DataSlug, DocumentId, ExtractorIdentity,
        GraphInputV1, LogicalEdgeRowV1, LogicalIndexName, LogicalNodeRowV1, PrepareCorpusInputV1,
        RootIdentity, WrapperId,
    };

    let mapping = || DataMappingV1::parse_json(br#"{"properties":{}}"#).unwrap();
    let document = || {
        DataDocumentV1::parse_source(DocumentId::from_str("doc-a").unwrap(), br#"{"x":1}"#).unwrap()
    };
    assert!(DataRouteInputV1::new(
        DataSlug::from_str("docs").unwrap(),
        LogicalIndexName::from_str("life-docs").unwrap(),
        mapping(),
        vec![document(), document()],
    )
    .is_err());

    let wrapper = || {
        CatalogWrapperV1::parse_public_source(WrapperId::from_str("wrap-a").unwrap(), br#"{"x":1}"#)
            .unwrap()
    };
    assert!(CatalogInputV1::new(
        CatalogMappingV1::parse_json(br#"{"properties":{}}"#).unwrap(),
        vec![wrapper(), wrapper()],
    )
    .is_err());

    let route = |index: &str| {
        DataRouteInputV1::new(
            DataSlug::from_str("docs").unwrap(),
            LogicalIndexName::from_str(index).unwrap(),
            mapping(),
            Vec::new(),
        )
        .unwrap()
    };
    let empty_catalog = || {
        CatalogInputV1::new(
            CatalogMappingV1::parse_json(br#"{"properties":{}}"#).unwrap(),
            Vec::new(),
        )
        .unwrap()
    };
    let empty_graph = || {
        GraphInputV1::new(
            BrainName::from_str("life").unwrap(),
            ExtractorIdentity::from_str("extractor@1").unwrap(),
            ExtractorConfigV1::parse_json(br#"{}"#).unwrap(),
            GraphEdgeMappingV1::parse_json(br#"{"properties":{}}"#).unwrap(),
            GraphNodeMappingV1::parse_json(br#"{"properties":{}}"#).unwrap(),
            Vec::new(),
            Vec::new(),
        )
        .unwrap()
    };
    assert!(PrepareCorpusInputV1::new(
        RootIdentity::from_str("/r").unwrap(),
        CorpusPrefix::from_str("life").unwrap(),
        CorpusIncarnationSeed::from_array([0; 32]),
        ManifestV1::parse_json(br#"{"entries":[],"format_version":1,"root_identity":"/r"}"#,)
            .unwrap(),
        vec![route("life-docs"), route("life-notes")],
        empty_catalog(),
        empty_graph(),
    )
    .is_err());

    let edge = || {
        LogicalEdgeRowV1::parse_json(br#"{"src":"a","dst":"b","type":"link","weight":1,"confidence":1,"valid_at":0,"created_at":0,"detector":"d@1","schema_version":1,"src_file":"a.md","evidence":{"quote":"b","source":"a.md","offset":0}}"#).unwrap()
    };
    assert!(GraphInputV1::new(
        BrainName::from_str("life").unwrap(),
        ExtractorIdentity::from_str("extractor@1").unwrap(),
        ExtractorConfigV1::parse_json(br#"{}"#).unwrap(),
        GraphEdgeMappingV1::parse_json(br#"{"properties":{}}"#).unwrap(),
        GraphNodeMappingV1::parse_json(br#"{"properties":{}}"#).unwrap(),
        vec![edge(), edge()],
        Vec::new(),
    )
    .is_err());

    let node = || {
        LogicalNodeRowV1::parse_json(br#"{"source_index":"life-docs","logical_node_id":"a","title":null,"preview":null,"path":"a.md"}"#).unwrap()
    };
    assert!(GraphInputV1::new(
        BrainName::from_str("life").unwrap(),
        ExtractorIdentity::from_str("extractor@1").unwrap(),
        ExtractorConfigV1::parse_json(br#"{}"#).unwrap(),
        GraphEdgeMappingV1::parse_json(br#"{"properties":{}}"#).unwrap(),
        GraphNodeMappingV1::parse_json(br#"{"properties":{}}"#).unwrap(),
        Vec::new(),
        vec![node(), node()],
    )
    .is_err());

    assert_runner("mutations::fresh_duplicate_input_matrix", 5, "parse_error");
}

fn fresh_content_variant(change: Option<usize>) -> xerj_corpus_publication::DurableBeginBundleV1 {
    use xerj_corpus_publication::{
        BrainName, CatalogInputV1, CatalogWrapperV1, CorpusIncarnationSeed, CorpusPrefix,
        DataDocumentV1, DataMappingV1, DataRouteInputV1, DataSlug, DocumentId,
        DurableBeginBundleV1, ExtractorIdentity, Generation, GraphInputV1, LogicalEdgeRowV1,
        LogicalIndexName, LogicalNodeRowV1, PlannedCorpusV1, PrepareCorpusInputV1,
        PreparedCorpusV1, RootIdentity, Sequence, SequenceTransitionV1, WrapperId,
    };

    let coordinated_id = if change == Some(1) { "doc-c" } else { "doc-a" };
    let manifest = ManifestV1::parse_json(
        format!(
            "{{\"entries\":[{{\"id\":\"{coordinated_id}\",\"path\":\"alpha.md\"}},{{\"id\":\"doc-b\",\"path\":\"beta.md\"}}],\"format_version\":1,\"root_identity\":\"/r\"}}"
        )
        .as_bytes(),
    )
    .unwrap();
    let data_mapping = if change == Some(6) {
        br#"{"properties":{"body":{"type":"text"},"changed":{"type":"keyword"}}}"#.as_slice()
    } else {
        br#"{"properties":{"body":{"type":"text"}}}"#.as_slice()
    };
    let data = DataRouteInputV1::new(
        DataSlug::from_str("docs").unwrap(),
        LogicalIndexName::from_str("life-docs").unwrap(),
        DataMappingV1::parse_json(data_mapping).unwrap(),
        vec![
            DataDocumentV1::parse_source(
                DocumentId::from_str(coordinated_id).unwrap(),
                if change == Some(0) {
                    br#"{"body":"Alpha changed.","path":"alpha.md"}"#
                } else {
                    br#"{"body":"Alpha links [[beta]].","path":"alpha.md"}"#
                },
            )
            .unwrap(),
            DataDocumentV1::parse_source(
                DocumentId::from_str("doc-b").unwrap(),
                br#"{"body":"Beta.","path":"beta.md"}"#,
            )
            .unwrap(),
        ],
    )
    .unwrap();
    let catalog_mapping = if change == Some(7) {
        br#"{"properties":{"canonical":{"type":"keyword"},"changed":{"type":"keyword"}}}"#
            .as_slice()
    } else {
        br#"{"properties":{"canonical":{"type":"keyword"}}}"#.as_slice()
    };
    let catalog = CatalogInputV1::new(
        CatalogMappingV1::parse_json(catalog_mapping).unwrap(),
        vec![CatalogWrapperV1::parse_public_source(
            WrapperId::from_str(if change == Some(3) {
                "wrap-b"
            } else {
                "wrap-a"
            })
            .unwrap(),
            if change == Some(2) {
                br#"{"kind":"file","path":"changed.md"}"#
            } else {
                br#"{"kind":"file","path":"alpha.md"}"#
            },
        )
        .unwrap()],
    )
    .unwrap();
    let edge = LogicalEdgeRowV1::parse_json(
        if change == Some(4) {
            br#"{"src":"doc-a","dst":"doc-b","type":"changed","weight":1,"confidence":1,"valid_at":0,"created_at":0,"detector":"wikilink@2","schema_version":1,"src_file":"alpha.md","evidence":{"quote":"[[beta]]","source":"alpha.md","offset":0}}"#
        } else if change == Some(1) {
            br#"{"src":"doc-c","dst":"doc-b","type":"wikilink","weight":1,"confidence":1,"valid_at":0,"created_at":0,"detector":"wikilink@2","schema_version":1,"src_file":"alpha.md","evidence":{"quote":"[[beta]]","source":"alpha.md","offset":0}}"#
        } else {
            br#"{"src":"doc-a","dst":"doc-b","type":"wikilink","weight":1,"confidence":1,"valid_at":0,"created_at":0,"detector":"wikilink@2","schema_version":1,"src_file":"alpha.md","evidence":{"quote":"[[beta]]","source":"alpha.md","offset":0}}"#
        },
    )
    .unwrap();
    let node_a = LogicalNodeRowV1::parse_json(
        if change == Some(5) {
            br#"{"source_index":"life-docs","logical_node_id":"doc-a","title":"Changed","preview":null,"path":"alpha.md"}"#
        } else if change == Some(1) {
            br#"{"source_index":"life-docs","logical_node_id":"doc-c","title":"Alpha","preview":null,"path":"alpha.md"}"#
        } else {
            br#"{"source_index":"life-docs","logical_node_id":"doc-a","title":"Alpha","preview":null,"path":"alpha.md"}"#
        },
    )
    .unwrap();
    let node_b = LogicalNodeRowV1::parse_json(
        br#"{"source_index":"life-docs","logical_node_id":"doc-b","title":null,"preview":"Beta.","path":"beta.md"}"#,
    )
    .unwrap();
    let edge_mapping = if change == Some(8) {
        br#"{"properties":{"physical_id":{"type":"keyword"},"changed":{"type":"keyword"}}}"#
            .as_slice()
    } else {
        br#"{"properties":{"physical_id":{"type":"keyword"}}}"#.as_slice()
    };
    let node_mapping = if change == Some(9) {
        br#"{"properties":{"physical_id":{"type":"keyword"},"changed":{"type":"keyword"}}}"#
            .as_slice()
    } else {
        br#"{"properties":{"physical_id":{"type":"keyword"}}}"#.as_slice()
    };
    let extractor = if change == Some(10) {
        br#"{"changed":true}"#.as_slice()
    } else {
        br#"{}"#.as_slice()
    };
    let graph = GraphInputV1::new(
        BrainName::from_str("life").unwrap(),
        ExtractorIdentity::from_str("extractor@1").unwrap(),
        ExtractorConfigV1::parse_json(extractor).unwrap(),
        GraphEdgeMappingV1::parse_json(edge_mapping).unwrap(),
        GraphNodeMappingV1::parse_json(node_mapping).unwrap(),
        vec![edge],
        vec![node_a, node_b],
    )
    .unwrap();
    let prepared = PreparedCorpusV1::prepare(
        PrepareCorpusInputV1::new(
            RootIdentity::from_str("/r").unwrap(),
            CorpusPrefix::from_str("life").unwrap(),
            CorpusIncarnationSeed::from_array(std::array::from_fn(|index| index as u8)),
            manifest,
            vec![data],
            catalog,
            graph,
        )
        .unwrap(),
    )
    .unwrap();
    let planned = PlannedCorpusV1::plan(
        prepared,
        SequenceTransitionV1::new(Sequence::new(0), Sequence::new(1)).unwrap(),
        Generation::new(if change == Some(11) { 7 } else { 1 }),
    )
    .unwrap();
    let owner = planned.desired_plan().owner().clone();
    DurableBeginBundleV1::build(ExpectedPublicationV1::absent(owner), planned).unwrap()
}

#[test]
fn fresh_logical_content_changed_chain() {
    let baseline = fresh_content_variant(None);
    for case in 0..12 {
        let changed = fresh_content_variant(Some(case));
        assert_ne!(
            changed.desired_plan().digest(),
            baseline.desired_plan().digest(),
            "fresh content case {case}"
        );
        assert_ne!(
            changed.sync_begin().digest(),
            baseline.sync_begin().digest(),
            "fresh content begin case {case}"
        );
    }
    assert_runner(
        "mutations::fresh_logical_content_changed_chain",
        12,
        "changed_chain",
    );
}

#[test]
fn u64_max_generation_succeeds() {
    let bundle = absent_bundle(u64::MAX);
    assert_eq!(bundle.desired_plan().generation().get(), u64::MAX);
    assert!(DesiredPublicationPlanV1::parse_canonical_preimage(
        bundle
            .desired_plan()
            .canonical_preimage()
            .canonical_preimage()
    )
    .is_ok());
    assert_runner("mutations::u64_max_generation_succeeds", 1, "success");
}

#[test]
fn physical_name_bound_matrix() {
    use xerj_corpus_publication::{PhysicalDataName, ResourceKey};

    let rendered_232 = format!(".{}", "a".repeat(231));
    assert_eq!(rendered_232.len(), 232);
    assert!(PhysicalDataName::from_str(&rendered_232).is_err());
    assert!(PhysicalDataName::from_str("xerj-visible-name").is_err());
    let resource_1025 = format!("data/.{}", "a".repeat(1019));
    assert_eq!(resource_1025.len(), 1025);
    assert!(ResourceKey::from_str(&resource_1025).is_err());
    assert_runner("mutations::physical_name_bound_matrix", 3, "parse_error");
}
