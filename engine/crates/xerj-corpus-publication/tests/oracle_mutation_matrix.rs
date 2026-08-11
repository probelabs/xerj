use std::{collections::BTreeSet, str::FromStr};

use serde_json::Value;
use xerj_corpus_publication::{
    CorpusIncarnationId, CorpusOwnerId, CorpusPublicationV1, DesiredPlanDigest,
    ExpectedPublicationDigest, ExpectedPublicationV1, ExtractorConfigDigest, ManifestDigest,
    MappingDigest, PreparedInputDigest, PublicationDigest, ReplayArtifactDigest, ReplaySetDigest,
    SyncBeginDigest, SyncBeginV1, TransactionId,
};

fn goldens() -> Value {
    serde_json::from_slice(include_bytes!("../testdata/review11-v1/goldens.json")).unwrap()
}

fn assert_rendered_mutations_reject<T>(baseline: &str, cross_domain: &str)
where
    T: FromStr,
    T::Err: std::fmt::Debug,
{
    assert!(baseline.parse::<T>().is_ok());
    let prefix_end = baseline.len() - 64;
    let variants = [
        format!("wrong-{}", &baseline[prefix_end..]),
        baseline.replacen("sha256", "sha512", 1),
        baseline[..baseline.len() - 1].to_owned(),
        format!(
            "{}{}",
            &baseline[..prefix_end],
            baseline[prefix_end..].to_uppercase()
        ),
        format!(" {baseline}"),
        format!("{baseline} "),
        format!("{baseline}\0"),
        format!("{cross_domain}{}", &baseline[prefix_end..]),
    ];
    for value in variants {
        assert!(
            value.parse::<T>().is_err(),
            "accepted rendered mutation: {value:?}"
        );
    }
}

fn duplicate_first_member(input: &str) -> Vec<u8> {
    let value: Value = serde_json::from_str(input).unwrap();
    let (name, original) = value.as_object().unwrap().iter().next().unwrap();
    let duplicate = format!(
        "{{{}:{},{}",
        serde_json::to_string(name).unwrap(),
        serde_json::to_string(original).unwrap(),
        &input[1..]
    );
    duplicate.into_bytes()
}

fn closed_top_level_mutations(input: &str) -> [Vec<u8>; 4] {
    let mut value: Value = serde_json::from_str(input).unwrap();
    let first = value.as_object().unwrap().keys().next().unwrap().to_owned();
    let mut unknown = value.clone();
    unknown
        .as_object_mut()
        .unwrap()
        .insert("unknown".to_owned(), Value::Bool(true));
    let mut missing = value.clone();
    missing.as_object_mut().unwrap().remove(&first);
    value.as_object_mut().unwrap().insert(first, Value::Null);
    [
        serde_json::to_vec(&unknown).unwrap(),
        serde_json::to_vec(&missing).unwrap(),
        serde_json::to_vec(&value).unwrap(),
        duplicate_first_member(input),
    ]
}

#[test]
fn all_thirteen_public_rendered_digest_types_reject_the_full_spelling_class() {
    let fixture = goldens();
    assert_rendered_mutations_reject::<CorpusOwnerId>(
        fixture["prepared"]["owner"]["rendered"].as_str().unwrap(),
        "xercpi1-sha256-",
    );
    assert_rendered_mutations_reject::<CorpusIncarnationId>(
        fixture["prepared"]["corpus_incarnation"]["rendered"]
            .as_str()
            .unwrap(),
        "xercpo1-sha256-",
    );
    assert_rendered_mutations_reject::<ManifestDigest>(
        fixture["prepared"]["manifest"]["rendered"]
            .as_str()
            .unwrap(),
        "xermap1-sha256-",
    );
    assert_rendered_mutations_reject::<ExtractorConfigDigest>(
        fixture["prepared"]["extractor_config"]["rendered"]
            .as_str()
            .unwrap(),
        "xerm1-sha256-",
    );
    assert_rendered_mutations_reject::<MappingDigest>(
        fixture["prepared"]["mappings"]["data"]["rendered"]
            .as_str()
            .unwrap(),
        "xerecfg1-sha256-",
    );
    assert_rendered_mutations_reject::<PreparedInputDigest>(
        fixture["prepared"]["prepared_input"]["rendered"]
            .as_str()
            .unwrap(),
        "xertx1-sha256-",
    );
    assert_rendered_mutations_reject::<TransactionId>(
        fixture["generation_1"]["transaction"]["rendered"]
            .as_str()
            .unwrap(),
        "xerpdi1-sha256-",
    );
    assert_rendered_mutations_reject::<ReplayArtifactDigest>(
        fixture["generation_1"]["artifacts"][0]["digest"]["rendered"]
            .as_str()
            .unwrap(),
        "xerrs1-sha256-",
    );
    assert_rendered_mutations_reject::<ReplaySetDigest>(
        fixture["generation_1"]["replay_set"]["rendered"]
            .as_str()
            .unwrap(),
        "xerra1-sha256-",
    );
    assert_rendered_mutations_reject::<DesiredPlanDigest>(
        fixture["generation_1"]["desired_plan"]["rendered"]
            .as_str()
            .unwrap(),
        "xercp1-sha256-",
    );
    assert_rendered_mutations_reject::<PublicationDigest>(
        fixture["prior_publication"]["publication"]["rendered"]
            .as_str()
            .unwrap(),
        "xerdp1-sha256-",
    );
    assert_rendered_mutations_reject::<ExpectedPublicationDigest>(
        fixture["expectations"]["absent"]["digest"]["rendered"]
            .as_str()
            .unwrap(),
        "xersb1-sha256-",
    );
    assert_rendered_mutations_reject::<SyncBeginDigest>(
        fixture["sync_begins"]["absent"]["digest"]["rendered"]
            .as_str()
            .unwrap(),
        "xerep1-sha256-",
    );
}

#[test]
fn publication_expectation_and_begin_reject_closed_top_level_member_mutations() {
    let fixture = goldens();
    let publication = fixture["prior_publication"]["publication"]["canonical_json"]
        .as_str()
        .unwrap();
    for mutation in closed_top_level_mutations(publication) {
        assert!(CorpusPublicationV1::parse_closed_json(&mutation).is_err());
    }
    for kind in ["absent", "present"] {
        let expectation = fixture["expectations"][kind]["canonical_json"]
            .as_str()
            .unwrap();
        for mutation in closed_top_level_mutations(expectation) {
            assert!(ExpectedPublicationV1::parse_closed_json(&mutation).is_err());
        }
        let begin = fixture["sync_begins"][kind]["canonical_json"]
            .as_str()
            .unwrap();
        for mutation in closed_top_level_mutations(begin) {
            assert!(SyncBeginV1::parse_closed_json(&mutation).is_err());
        }
    }
}

#[test]
fn mutation_inventory_is_concrete_unique_and_uses_closed_outcomes() {
    let matrix: Value =
        serde_json::from_slice(include_bytes!("../testdata/review11-v1/mutations.json")).unwrap();
    assert_eq!(matrix["format_version"], 2);
    let rows = matrix["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 30);
    assert_eq!(matrix["summary"]["row_count"], 30);
    assert_eq!(matrix["summary"]["case_count"], 28_797);
    let gates = matrix["execution_evidence"]["gates"].as_object().unwrap();
    assert_eq!(gates.len(), 4);
    let integration_rows = rows
        .iter()
        .filter(|row| row["runner"].as_str().unwrap().starts_with("mutations::"))
        .collect::<Vec<_>>();
    let unit_rows = rows
        .iter()
        .filter(|row| row["runner"].as_str().unwrap().starts_with("codec::"))
        .collect::<Vec<_>>();
    let compile_rows = rows
        .iter()
        .filter(|row| row["runner"].as_str().unwrap().starts_with("ui::"))
        .collect::<Vec<_>>();
    let nul_rows = rows
        .iter()
        .filter(|row| {
            row["runner"]
                .as_str()
                .unwrap()
                .starts_with("resource_key_nul::")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        gates["integration"]["expected"]["runner_functions"],
        integration_rows.len() as u64
    );
    assert_eq!(
        gates["integration"]["expected"]["test_functions_including_registry"],
        integration_rows.len() as u64 + 1
    );
    assert_eq!(
        gates["integration"]["expected"]["runner_cases"],
        integration_rows
            .iter()
            .map(|row| row["case_count"].as_u64().unwrap())
            .sum::<u64>()
    );
    assert_eq!(gates["integration"]["expected"]["registry_rows"], 30);
    assert_eq!(gates["integration"]["expected"]["registry_cases"], 28_797);
    for (gate, selected, count_key) in [
        ("unit", &unit_rows, "cases"),
        ("compile", &compile_rows, "fixtures"),
        ("resource_key_nul", &nul_rows, "cases"),
    ] {
        assert_eq!(gates[gate]["expected"]["rows"], selected.len() as u64);
        assert_eq!(
            gates[gate]["expected"][count_key],
            selected
                .iter()
                .map(|row| row["case_count"].as_u64().unwrap())
                .sum::<u64>()
        );
    }
    let integration = &gates["integration"];
    let integration_passed = integration["observed"]["failed_test_functions"] == 0
        && integration["observed"]["passed_test_functions"]
            == integration["expected"]["test_functions_including_registry"]
        && integration["observed"]["registry_exact"] == true
        && integration["observed"]["registry_rows"] == integration["expected"]["registry_rows"]
        && integration["observed"]["registry_cases"] == integration["expected"]["registry_cases"];
    let unit_passed = gates["unit"]["observed"]["failed_cases"] == 0
        && gates["unit"]["observed"]["passed_cases"] == gates["unit"]["expected"]["cases"];
    let compile_passed = gates["compile"]["observed"]["failed_fixtures"] == 0
        && gates["compile"]["observed"]["passed_fixtures"]
            == gates["compile"]["expected"]["fixtures"];
    let nul_passed = gates["resource_key_nul"]["observed"]["failed_cases"] == 0
        && gates["resource_key_nul"]["observed"]["passed_cases"]
            == gates["resource_key_nul"]["expected"]["cases"];
    let derived_complete = integration_passed && unit_passed && compile_passed && nul_passed;
    assert_eq!(matrix["execution_complete"], derived_complete);
    assert_eq!(
        matrix["execution_evidence"]["status"],
        if derived_complete {
            "complete"
        } else {
            "incomplete"
        }
    );
    if derived_complete {
        assert!(matrix["execution_blocker"].is_null());
    } else {
        let blockers = matrix["execution_blocker"]
            .as_array()
            .unwrap()
            .iter()
            .map(|gate| gate.as_str().unwrap())
            .collect::<BTreeSet<_>>();
        let derived_blockers = gates
            .iter()
            .filter(|(_, gate)| gate["status"] == "incomplete")
            .map(|(name, _)| name.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(blockers, derived_blockers);
    }
    let mut ids = BTreeSet::new();
    let mut total = 0;
    for row in rows {
        let expected_gate = match row["runner"].as_str().unwrap() {
            runner if runner.starts_with("mutations::") => "integration",
            runner if runner.starts_with("codec::") => "unit",
            runner if runner.starts_with("ui::") => "compile",
            runner if runner.starts_with("resource_key_nul::") => "resource_key_nul",
            runner => panic!("unclassified runner: {runner}"),
        };
        let gate = row["execution_gate"].as_str().unwrap();
        assert_eq!(gate, expected_gate);
        assert_eq!(
            row["execution_status"],
            if gates[gate]["status"] == "passed" {
                "executed_passed"
            } else {
                "incomplete"
            }
        );
        assert!(ids.insert(row["id"].as_str().unwrap()));
        assert!(!row["baseline"].as_str().unwrap().is_empty());
        assert!(!row["mutation"].as_str().unwrap().is_empty());
        assert!(matches!(
            row["outcome"].as_str().unwrap(),
            "parse_error"
                | "changed_chain"
                | "unchanged"
                | "parse_error_or_changed_chain"
                | "compile_error"
                | "success"
        ));
        let count = row["case_count"].as_u64().unwrap();
        total += count;
        let explicit = row["cases"].as_array().unwrap();
        if explicit.is_empty() {
            let ranges = row["case_ranges"].as_array().unwrap();
            assert_eq!(
                ranges
                    .iter()
                    .map(|range| range["count"].as_u64().unwrap())
                    .sum::<u64>(),
                count,
                "{}",
                row["id"]
            );
            for range in ranges {
                assert_eq!(
                    range["last_offset"].as_u64().unwrap()
                        - range["first_offset"].as_u64().unwrap()
                        + 1,
                    range["count"].as_u64().unwrap()
                );
            }
        } else {
            assert_eq!(explicit.len() as u64, count, "{}", row["id"]);
            let names = explicit
                .iter()
                .map(|case| case.as_str().unwrap())
                .collect::<BTreeSet<_>>();
            assert_eq!(
                names.len(),
                explicit.len(),
                "duplicate case in {}",
                row["id"]
            );
        }
    }
    assert_eq!(total, 28_797);
    for required in [
        "digest-rendering-cartesian",
        "closed-json-cartesian",
        "standalone-binary-byte-flips",
        "persisted-class-byte-flips",
        "persisted-replay-byte-flips",
        "mapping-reservation-persisted-matrix",
        "quota-persisted-matrix",
        "strict-replay-boundary-matrix",
        "strict-replay-action-matrix",
        "strict-replay-content-join-matrix",
        "persisted-cross-file-join-matrix",
        "publication-expectation-begin-matrix",
        "compile-time-semantic-swaps",
    ] {
        assert!(ids.contains(required), "missing mutation class {required}");
    }

    let row = |id: &str| rows.iter().find(|row| row["id"] == id).unwrap();
    let quota_cases = row("quota-persisted-matrix")["cases"].as_array().unwrap();
    assert_eq!(quota_cases.len(), 17);
    assert!(!quota_cases
        .iter()
        .any(|case| case == "stage-add-resource-overflow"));
    let ordering_cases = row("ordering-persisted-matrix")["cases"]
        .as_array()
        .unwrap();
    assert_eq!(ordering_cases.len(), 10);
    for non_persisted_case in [
        "edge-physical-id-set:descending-rendered-id",
        "node-physical-id-set:logical-order-instead-of-rendered-order",
    ] {
        assert!(!ordering_cases.iter().any(|case| case == non_persisted_case));
    }
    let private_cases = row("checked-arithmetic-private-matrix")["cases"]
        .as_array()
        .unwrap();
    assert!(private_cases
        .iter()
        .any(|case| case == "resource-times-4096-overflow"));
    assert!(private_cases
        .iter()
        .any(|case| case == "stage-add-third-overflow"));
}
