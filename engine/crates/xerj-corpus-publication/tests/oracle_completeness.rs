#[path = "support/mod.rs"]
mod support;

use serde_json::Value;
use sha2::{Digest, Sha256};
use xerj_corpus_publication::{ProjectionKind, ReplayArtifactV1};

fn fixture() -> Value {
    serde_json::from_slice(include_bytes!("../testdata/review11-v1/goldens.json")).unwrap()
}

fn decode(value: &Value, field: &str) -> Vec<u8> {
    support::reference_codec::decode_padded_base64(value[field].as_str().unwrap())
}

fn assert_all_complete_vectors(value: &Value, path: &str, count: &mut usize) {
    match value {
        Value::Object(fields) => {
            if fields.contains_key("preimage_base64") {
                let bytes = decode(value, "preimage_base64");
                assert_eq!(
                    value["preimage_length"].as_u64().unwrap(),
                    bytes.len() as u64,
                    "{path}/preimage_length"
                );
                let sha = support::reference_codec::sha256_hex(&bytes);
                assert_eq!(value["sha256"].as_str().unwrap(), sha, "{path}/sha256");
                if let Some(rendered) = value.get("rendered").and_then(Value::as_str) {
                    let prefix_end = rendered.len() - 64;
                    assert_eq!(rendered, format!("{}{}", &rendered[..prefix_end], sha));
                }
                *count += 1;
            }
            if fields.contains_key("body_base64") {
                let bytes = decode(value, "body_base64");
                assert_eq!(
                    value["body_length"].as_u64().unwrap(),
                    bytes.len() as u64,
                    "{path}/body_length"
                );
                assert_eq!(
                    value["canonical_json"].as_str().unwrap().as_bytes(),
                    bytes,
                    "{path}/canonical_json"
                );
            }
            if fields.contains_key("canonical_json_base64") {
                let bytes = decode(value, "canonical_json_base64");
                assert_eq!(
                    value["canonical_json_length"].as_u64().unwrap(),
                    bytes.len() as u64,
                    "{path}/canonical_json_length"
                );
                assert_eq!(
                    value["canonical_json"].as_str().unwrap().as_bytes(),
                    bytes,
                    "{path}/canonical_json"
                );
            }
            if fields.contains_key("bytes_base64") {
                let bytes = decode(value, "bytes_base64");
                assert_eq!(
                    value["byte_length"].as_u64().unwrap(),
                    bytes.len() as u64,
                    "{path}/byte_length"
                );
                assert_eq!(
                    value["raw_sha256"].as_str().unwrap(),
                    support::reference_codec::sha256_hex(&bytes),
                    "{path}/raw_sha256"
                );
            }
            for (name, child) in fields {
                assert_all_complete_vectors(child, &format!("{path}/{name}"), count);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                assert_all_complete_vectors(child, &format!("{path}/{index}"), count);
            }
        }
        _ => {}
    }
}

fn artifact_action_ids(bytes: &[u8]) -> Vec<String> {
    bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .chunks_exact(2)
        .map(|pair| {
            let value: Value = serde_json::from_slice(pair[0]).unwrap();
            value["index"]["_id"].as_str().unwrap().to_owned()
        })
        .collect()
}

fn artifact_for(artifacts: &[ReplayArtifactV1], projection: ProjectionKind) -> &ReplayArtifactV1 {
    let mut matches = artifacts
        .iter()
        .filter(|artifact| artifact.projection_kind() == projection);
    let artifact = matches.next().expect("projection replay artifact");
    assert!(matches.next().is_none(), "projection must be unique");
    artifact
}

#[test]
fn every_checked_in_binary_or_json_oracle_is_complete_and_self_consistent() {
    let fixture = fixture();
    let mut count = 0;
    assert_all_complete_vectors(&fixture, "$", &mut count);
    assert!(
        count >= 150,
        "unexpectedly sparse complete-vector coverage: {count}"
    );
    assert!(fixture["coverage"]["not_complete"]
        .as_array()
        .unwrap()
        .is_empty());

    let provenance: Value =
        serde_json::from_slice(include_bytes!("../testdata/review11-v1/provenance.json")).unwrap();
    let mutations: Value =
        serde_json::from_slice(include_bytes!("../testdata/review11-v1/mutations.json")).unwrap();
    let incomplete_scope = provenance["incomplete_scope"].as_array().unwrap();
    assert!(incomplete_scope.is_empty());
    assert_eq!(
        provenance["candidate_source"]["status"],
        "final_attested_repaired_source_commit"
    );
    assert_eq!(
        provenance["candidate_source"]["base_commit"],
        "aa142d6772a046baa9d5728328737020d3d05818"
    );
    assert_eq!(
        provenance["candidate_source"]["base_tree"],
        "2f9e469b1f1e12ab9005e0f666ddb1ff2cd680b9"
    );
    assert_eq!(
        provenance["candidate_source"]["candidate_commit"],
        "fc04d9aba39ced1dfcff78bce67ba3b39a660a4e"
    );
    assert_eq!(
        provenance["candidate_source"]["candidate_tree"],
        "13c75a660b63629e53860d5d8792266862f7f835"
    );
    assert_eq!(
        provenance["candidate_source"]["claim"],
        "This final attestation supersedes the earlier attestation and pins the exact repaired source commit and tree. The final provenance-attestation commit changes evidence metadata and oracle assertions only; it intentionally does not and cannot self-pin its own commit or tree."
    );
    assert_eq!(
        provenance["attestation_commit"]["changes_evidence_metadata_only"],
        true
    );
    assert_eq!(provenance["attestation_commit"]["self_pins"], false);
    assert_eq!(
        provenance["attestation_commit"]["supersedes_attestation_commit"],
        "5eaecbcecbfebaece8c03a83bab4250a5a95ea9d"
    );
    assert_eq!(
        provenance["attestation_commit"]["superseded_candidate_commit"],
        "a214f1587df2d39f38e1017b4ebe4766715e3716"
    );
    assert_eq!(
        provenance["attestation_commit"]["superseded_candidate_tree"],
        "3eae5b0efbac8598d9939c76272e585874124a4a"
    );
    assert_eq!(
        provenance["attestation_commit"]["statement"],
        "The final attestation commit is deliberately outside the pinned repaired source identity. It supersedes the earlier attestation, records the already-committed repair identity, and updates only the generated evidence metadata and assertions that verify it."
    );
    assert_eq!(
        provenance["attestation_commit"]["pinned_preceding_commit"],
        provenance["candidate_source"]["candidate_commit"]
    );
    assert_eq!(
        provenance["attestation_commit"]["pinned_preceding_tree"],
        provenance["candidate_source"]["candidate_tree"]
    );
    let execution_complete = mutations["execution_complete"].as_bool().unwrap();
    let execution_status = if execution_complete {
        "complete"
    } else {
        "incomplete"
    };
    assert_eq!(
        provenance["mutation_execution"]["execution_complete"],
        execution_complete
    );
    assert_eq!(provenance["mutation_execution"]["status"], execution_status);
    assert_eq!(provenance["mutation_execution"]["ledger_rows"], 30);
    assert_eq!(provenance["mutation_execution"]["ledger_cases"], 28_797);
    assert_eq!(mutations["format_version"], 2);
    assert_eq!(mutations["rows"].as_array().unwrap().len(), 30);
    assert_eq!(mutations["summary"]["row_count"], 30);
    assert_eq!(mutations["summary"]["case_count"], 28_797);
    assert_eq!(mutations["execution_evidence"]["status"], execution_status);
    assert_eq!(
        mutations["execution_evidence"]["declared_case_count"],
        28_797
    );
    assert_eq!(
        provenance["mutation_execution"]["gates"],
        mutations["execution_evidence"]["gates"]
    );
    let generator = include_bytes!("../testdata/review11-v1/generate.py");
    assert_eq!(
        provenance["independent_generator"]["bytes"]
            .as_u64()
            .unwrap(),
        generator.len() as u64
    );
    assert_eq!(
        provenance["independent_generator"]["sha256"]
            .as_str()
            .unwrap(),
        format!("{:x}", Sha256::digest(generator))
    );
    let reference_encoder = include_bytes!("support/reference_codec.rs");
    assert_eq!(
        provenance["reference_encoder"]["status"],
        "pinned_in_repaired_source_commit; unchanged_by_final_attestation"
    );
    assert_eq!(
        provenance["reference_encoder"]["sha256"].as_str().unwrap(),
        format!("{:x}", Sha256::digest(reference_encoder))
    );
}

#[test]
fn expectation_and_sync_begin_bodies_are_independently_framed_and_padded() {
    let fixture = fixture();
    for kind in ["absent", "present"] {
        let expected = &fixture["expectations"][kind];
        let binary_body = decode(expected, "binary_body_base64");
        assert_eq!(
            expected["binary_body_length"].as_u64().unwrap(),
            binary_body.len() as u64
        );
        let mut expected_preimage = b"xerj-expected-publication-v1\0".to_vec();
        expected_preimage.extend_from_slice(&binary_body);
        assert_eq!(
            decode(&expected["digest"], "preimage_base64"),
            expected_preimage
        );
    }

    for kind in ["absent", "present"] {
        let begin = &fixture["sync_begins"][kind];
        let binary_body = decode(begin, "binary_body_base64");
        assert_eq!(
            begin["binary_body_length"].as_u64().unwrap(),
            binary_body.len() as u64
        );
        let mut begin_preimage = b"xerj-sync-begin-v1\0".to_vec();
        begin_preimage.extend_from_slice(&binary_body);
        assert_eq!(decode(&begin["digest"], "preimage_base64"), begin_preimage);
        let transition = if kind == "absent" {
            &fixture["generation_1"]
        } else {
            &fixture["present_transition"]
        };
        assert_eq!(
            begin["canonical_plan_base64"].as_str().unwrap(),
            transition["desired_plan"]["preimage_base64"]
                .as_str()
                .unwrap()
        );
    }
}

#[test]
fn generation_seven_is_pinned_descendant_by_descendant() {
    let fixture = fixture();
    let assertions = fixture["generation_7_invariants"]["descendant_assertions"]
        .as_array()
        .unwrap();
    let paths = assertions
        .iter()
        .map(|row| row["path"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        paths,
        vec![
            "transaction",
            "name_components.owner",
            "name_components.slug",
            "name_components.stage",
            "generation_id",
            "physical_data_name",
            "catalog_generation_incarnation",
            "graph_token",
            "edge_physical_ids",
            "node_physical_ids",
            "edge_physical_id_set",
            "node_physical_id_set",
            "data_projection",
            "catalog_projection",
            "graph_projection",
            "artifacts",
            "replay_set",
            "reserved_resource_keys",
            "desired_plan",
            "quota.mapping_record_bodies_base64",
            "quota.total",
        ]
    );
    assert!(assertions.iter().all(|row| row["satisfied"] == true));

    let bundle = support::absent_bundle(7);
    let oracle = &fixture["generation_7"];
    assert_eq!(
        bundle
            .desired_plan()
            .canonical_preimage()
            .canonical_preimage(),
        decode(&oracle["desired_plan"], "preimage_base64")
    );
    assert_eq!(
        bundle.desired_plan().transaction_id().as_rendered_str(),
        oracle["transaction"]["rendered"].as_str().unwrap()
    );
    assert_eq!(
        bundle.desired_plan().replay_set_digest().as_rendered_str(),
        oracle["replay_set"]["rendered"].as_str().unwrap()
    );

    for (actual, expected) in bundle
        .replay_artifacts()
        .iter()
        .zip(oracle["artifacts"].as_array().unwrap())
    {
        assert_eq!(
            actual.bytes().artifact_bytes(),
            decode(expected, "bytes_base64")
        );
        assert_eq!(
            actual.digest().as_rendered_str(),
            expected["digest"]["rendered"].as_str().unwrap()
        );
    }
    let artifacts = bundle.replay_artifacts();
    let data_artifact = artifact_for(artifacts, ProjectionKind::Data);
    let catalog_artifact = artifact_for(artifacts, ProjectionKind::Catalog);
    let edge_artifact = artifact_for(artifacts, ProjectionKind::GraphEdge);
    let node_artifact = artifact_for(artifacts, ProjectionKind::GraphNode);
    let data_resource = data_artifact.resource_key().as_protocol_str();
    assert_eq!(
        data_resource.strip_prefix("data/").unwrap(),
        oracle["physical_data_name"].as_str().unwrap()
    );
    let catalog_generation = catalog_artifact
        .resource_key()
        .as_protocol_str()
        .rsplit('/')
        .next()
        .unwrap();
    assert_eq!(catalog_generation, oracle["generation_id"]["rendered"]);
    let graph_token = edge_artifact
        .resource_key()
        .as_protocol_str()
        .rsplit('/')
        .next()
        .unwrap();
    assert_eq!(graph_token, oracle["graph_token"]["rendered"]);
    assert_eq!(
        artifact_action_ids(edge_artifact.bytes().artifact_bytes()),
        oracle["edge_physical_ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["rendered"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        artifact_action_ids(node_artifact.bytes().artifact_bytes()),
        oracle["node_physical_ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["rendered"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    );
}
