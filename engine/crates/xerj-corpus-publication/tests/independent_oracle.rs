mod support;

use serde_json::Value;
use support::{absent_bundle, present_bundle};
use xerj_corpus_publication::{CorpusPublicationV1, ExpectedPublicationV1, SyncBeginV1};

fn fixture() -> Value {
    serde_json::from_slice(include_bytes!("../testdata/review11-v1/goldens.json")).unwrap()
}

fn decoded(value: &Value, key: &str) -> Vec<u8> {
    support::reference_codec::decode_padded_base64(value[key].as_str().unwrap())
}

fn assert_vector(value: &Value, bytes: &[u8], prefix: &str) {
    assert_eq!(
        value["preimage_length"].as_u64().unwrap(),
        bytes.len() as u64
    );
    assert_eq!(decoded(value, "preimage_base64"), bytes);
    assert_eq!(
        value["sha256"].as_str().unwrap(),
        support::reference_codec::sha256_hex(bytes)
    );
    assert_eq!(
        value["rendered"].as_str().unwrap(),
        support::reference_codec::rendered(prefix, bytes)
    );
}

fn assert_generation(bundle: &xerj_corpus_publication::DurableBeginBundleV1, value: &Value) {
    let prepared = bundle
        .prepared_input()
        .canonical_preimage()
        .canonical_preimage();
    let fixture = fixture();
    assert_vector(
        &fixture["prepared"]["prepared_input"],
        prepared,
        "xerpdi1-sha256-",
    );

    let plan = bundle
        .desired_plan()
        .canonical_preimage()
        .canonical_preimage();
    assert_vector(&value["desired_plan"], plan, "xerdp1-sha256-");
    assert_eq!(
        bundle.desired_plan().transaction_id().as_rendered_str(),
        value["transaction"]["rendered"].as_str().unwrap()
    );
    assert_eq!(
        bundle.desired_plan().replay_set_digest().as_rendered_str(),
        value["replay_set"]["rendered"].as_str().unwrap()
    );
    assert_eq!(
        bundle.desired_plan().quota_charge(),
        value["quota"]["total"].as_u64().unwrap()
    );
    assert_eq!(
        bundle.desired_plan().generation().get(),
        value["generation"].as_u64().unwrap()
    );
    assert_eq!(
        bundle
            .desired_plan()
            .reserved_resource_keys()
            .iter()
            .map(|item| item.as_protocol_str())
            .collect::<Vec<_>>(),
        value["reserved_resource_keys"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item.as_str().unwrap())
            .collect::<Vec<_>>()
    );

    let expected_artifacts = value["artifacts"].as_array().unwrap();
    assert_eq!(bundle.replay_artifacts().len(), expected_artifacts.len());
    for (actual, expected) in bundle.replay_artifacts().iter().zip(expected_artifacts) {
        let bytes = actual.bytes().artifact_bytes();
        assert_eq!(
            actual.kind().to_string(),
            expected["kind"].as_str().unwrap()
        );
        assert_eq!(
            actual.projection_kind().to_string(),
            expected["projection_kind"].as_str().unwrap()
        );
        assert_eq!(
            actual.resource_key().as_protocol_str(),
            expected["resource_key"].as_str().unwrap()
        );
        assert_eq!(
            actual.byte_length(),
            expected["byte_length"].as_u64().unwrap()
        );
        assert_eq!(
            actual.operation_count(),
            expected["operation_count"].as_u64().unwrap()
        );
        assert_eq!(
            actual.digest().as_rendered_str(),
            expected["digest"]["rendered"].as_str().unwrap()
        );
        assert_eq!(
            bytes,
            support::reference_codec::decode_padded_base64(
                expected["bytes_base64"].as_str().unwrap()
            )
        );
        assert_eq!(
            support::reference_codec::sha256_hex(bytes),
            expected["raw_sha256"].as_str().unwrap()
        );
        let preimage = decoded(&expected["digest"], "preimage_base64");
        assert_vector(&expected["digest"], &preimage, "xerra1-sha256-");
    }
}

fn assert_begin(bundle: &xerj_corpus_publication::DurableBeginBundleV1, expected: &Value) {
    let bytes = bundle.sync_begin().canonical_json().canonical_json();
    assert_eq!(
        bytes,
        expected["canonical_json"].as_str().unwrap().as_bytes()
    );
    assert_eq!(
        bytes.len() as u64,
        expected["body_length"].as_u64().unwrap()
    );
    assert_eq!(
        support::reference_codec::padded_base64(bytes),
        expected["body_base64"].as_str().unwrap()
    );
    assert_eq!(
        bundle.sync_begin().digest().as_rendered_str(),
        expected["digest"]["rendered"].as_str().unwrap()
    );
    let parsed = SyncBeginV1::parse_closed_json(bytes).unwrap();
    assert_eq!(
        parsed.canonical_json(),
        bundle.sync_begin().canonical_json()
    );
    assert_eq!(parsed.digest(), bundle.sync_begin().digest());
}

#[test]
fn independent_oracle_pins_primary_and_generation_seven_complete_bytes() {
    let fixture = fixture();
    let one = absent_bundle(1);
    let seven = absent_bundle(7);
    assert_generation(&one, &fixture["generation_1"]);
    assert_generation(&seven, &fixture["generation_7"]);
    assert_begin(&one, &fixture["sync_begins"]["absent"]);
    assert_eq!(
        one.desired_plan().transaction_id(),
        seven.desired_plan().transaction_id(),
        "generation is not a transaction input"
    );
    assert_eq!(
        fixture["generation_7_invariants"]["changed_descendants"]
            .as_array()
            .unwrap()
            .len(),
        15
    );
}

#[test]
fn independent_oracle_pins_publication_present_expectation_and_begin() {
    let fixture = fixture();
    let publication_bytes = include_bytes!("../testdata/review11-v1/publication.json");
    let publication = CorpusPublicationV1::parse_closed_json(publication_bytes).unwrap();
    let publication_fixture = &fixture["prior_publication"]["publication"];
    assert_eq!(
        publication.canonical_json().canonical_json(),
        publication_fixture["canonical_json"]
            .as_str()
            .unwrap()
            .as_bytes()
    );
    assert_eq!(
        publication.digest().as_rendered_str(),
        publication_fixture["rendered"].as_str().unwrap()
    );
    let publication_preimage = decoded(publication_fixture, "preimage_base64");
    assert_vector(publication_fixture, &publication_preimage, "xercp1-sha256-");

    let expected = ExpectedPublicationV1::present(publication).unwrap();
    let expected_fixture = &fixture["expectations"]["present"];
    assert_eq!(
        expected.canonical_json().canonical_json(),
        expected_fixture["canonical_json"]
            .as_str()
            .unwrap()
            .as_bytes()
    );
    assert_eq!(
        expected.digest().as_rendered_str(),
        expected_fixture["digest"]["rendered"].as_str().unwrap()
    );

    let bundle = present_bundle();
    assert_generation(&bundle, &fixture["present_transition"]);
    assert_begin(&bundle, &fixture["sync_begins"]["present"]);
}

#[test]
fn every_oracle_base64_field_is_padded_and_exactly_reencodes() {
    fn visit(value: &Value, path: &str) {
        match value {
            Value::Object(fields) => {
                for (name, child) in fields {
                    let next = format!("{path}/{name}");
                    if name.ends_with("_base64") && child.is_string() {
                        let encoded = child.as_str().expect("string checked");
                        let decoded = support::reference_codec::decode_padded_base64(encoded);
                        assert_eq!(
                            support::reference_codec::padded_base64(&decoded),
                            encoded,
                            "{next}"
                        );
                    }
                    visit(child, &next);
                }
            }
            Value::Array(items) => {
                for (index, child) in items.iter().enumerate() {
                    visit(child, &format!("{path}/{index}"));
                }
            }
            _ => {}
        }
    }
    visit(&fixture(), "$");
}

#[test]
fn excluded_or_incomplete_oracle_rows_always_include_a_reason() {
    let fixture = fixture();
    for section in ["not_complete", "not_in_slice"] {
        for row in fixture["coverage"][section].as_array().unwrap() {
            assert!(!row["requirement"].as_str().unwrap().is_empty());
            assert!(!row["reason"].as_str().unwrap().is_empty());
        }
    }
}
