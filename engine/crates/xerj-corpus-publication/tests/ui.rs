use std::{collections::BTreeSet, fs, path::Path};

const COMPILE_FAIL_FIXTURES: [(&str, &str); 24] = [
    ("root-for-prefix", "prefix_for_root"),
    ("prefix-for-root", "swap_root_prefix"),
    ("slug-for-logical-index", "logical_index_for_slug"),
    ("logical-index-for-slug", "swap_slug_logical_index"),
    (
        "data-mapping-for-catalog-mapping",
        "catalog_mapping_for_data_mapping",
    ),
    (
        "catalog-mapping-for-data-mapping",
        "swap_projection_mapping",
    ),
    ("document-id-for-wrapper-id", "wrapper_id_for_document_id"),
    ("wrapper-id-for-document-id", "swap_document_wrapper_id"),
    (
        "brain-for-extractor-identity",
        "extractor_identity_for_brain",
    ),
    ("extractor-identity-for-brain", "swap_brain_extractor"),
    ("sequence-for-generation", "generation_for_sequence"),
    ("generation-for-sequence", "swap_sequence_generation"),
    ("later-target-value-in-prepare", "later_value_in_prepare"),
    (
        "later-plan-value-in-transaction",
        "later_value_in_transaction",
    ),
    (
        "construct-controlled-byte-wrapper",
        "construct_canonical_bytes",
    ),
    (
        "construct-mapping-reservation",
        "construct_mapping_reservation",
    ),
    ("mutate-mapping-json", "mutate_mapping_json"),
    (
        "swap-persisted-prepared-and-plan-wrappers",
        "swap_persisted_byte_classes",
    ),
    ("inspect-persisted-wrapper-bytes", "inspect_persisted_bytes"),
    ("access-private-codec", "private_codec"),
    ("access-raw-digest-bytes", "raw_digest_bytes"),
    ("cross-digest-domain-substitution", "cross_digest_domain"),
    ("serde-digest", "serde_digest"),
    ("construct-private-scalar-field", "private_fields"),
];

fn fixture_stems_with_extension(directory: &Path, extension: &str) -> BTreeSet<String> {
    fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some(extension))
        .map(|path| path.file_stem().unwrap().to_str().unwrap().to_owned())
        .collect()
}

#[test]
fn public_surface_and_privacy_contract() {
    let ledger: serde_json::Value =
        serde_json::from_slice(include_bytes!("../testdata/review11-v1/mutations.json")).unwrap();
    let row = ledger["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["runner"] == "ui::public_surface_and_privacy_contract")
        .unwrap();
    let ledger_cases = row["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| case.as_str().unwrap().to_owned())
        .collect::<BTreeSet<_>>();
    let mapped_cases = COMPILE_FAIL_FIXTURES
        .iter()
        .map(|(case, _)| (*case).to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(row["case_count"].as_u64(), Some(24));
    assert_eq!(ledger_cases.len(), 24);
    assert_eq!(ledger_cases, mapped_cases);

    let mapped_fixtures = COMPILE_FAIL_FIXTURES
        .iter()
        .map(|(_, fixture)| (*fixture).to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(mapped_fixtures.len(), 24);
    let fixture_directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/ui/fail");
    assert_eq!(
        fixture_stems_with_extension(&fixture_directory, "rs"),
        mapped_fixtures
    );
    assert_eq!(
        fixture_stems_with_extension(&fixture_directory, "stderr"),
        mapped_fixtures
    );

    let t = trybuild::TestCases::new();
    t.pass("tests/ui/pass/*.rs");
    for (_, fixture) in COMPILE_FAIL_FIXTURES {
        t.compile_fail(format!("tests/ui/fail/{fixture}.rs"));
    }
}
