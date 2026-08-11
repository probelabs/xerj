mod support;
use proptest::prelude::*;
use support::{absent_bundle, absent_bundle_with};
use xerj_corpus_publication::{
    CatalogMappingV1, CorpusPublicationV1, DataMappingV1, DesiredPublicationPlanV1,
    ExtractorConfigV1, GraphEdgeMappingV1, GraphNodeMappingV1, LogicalEdgeRowV1, LogicalNodeRowV1,
    ManifestV1, PreparedInputV1, ProjectionKind, SyncBeginV1,
};

proptest! {
    #[test]
    fn generated_u64_generation_roundtrips(generation in any::<u64>()) {
        let bundle = absent_bundle(generation);
        prop_assert_eq!(DesiredPublicationPlanV1::parse_canonical_preimage(bundle.desired_plan().canonical_preimage().canonical_preimage()).unwrap().generation().get(), generation);
        prop_assert!(PreparedInputV1::parse_canonical_preimage(bundle.prepared_input().canonical_preimage().canonical_preimage()).is_ok());
        prop_assert!(SyncBeginV1::parse_closed_json(bundle.sync_begin().canonical_json().canonical_json()).is_ok());
    }
}

#[test]
fn open_payloads_retain_arbitrary_nested_members_and_change_the_chain() {
    let baseline = absent_bundle(1);
    let document = absent_bundle_with(
        1,
        br#"{"z":{"nested":true},"path":"alpha.md","body":"Alpha links [[beta]].","a":7}"#,
        br#"{"path":"alpha.md","kind":"file"}"#,
        b"{}",
    );
    assert!(std::str::from_utf8(
        document
            .replay_artifacts()
            .iter()
            .find(|artifact| artifact.projection_kind() == ProjectionKind::Data)
            .unwrap()
            .bytes()
            .artifact_bytes(),
    )
    .unwrap()
    .contains("\"a\":7"));
    assert_ne!(
        document.desired_plan().digest(),
        baseline.desired_plan().digest()
    );
    let catalog = absent_bundle_with(
        1,
        br#"{"path":"alpha.md","body":"Alpha links [[beta]]."}"#,
        br#"{"z":{"custom":true},"path":"alpha.md","kind":"file","a":7}"#,
        b"{}",
    );
    assert!(std::str::from_utf8(
        catalog
            .replay_artifacts()
            .iter()
            .find(|artifact| artifact.projection_kind() == ProjectionKind::Catalog)
            .unwrap()
            .bytes()
            .artifact_bytes(),
    )
    .unwrap()
    .contains("\"a\":7"));
    assert_ne!(
        catalog.desired_plan().digest(),
        baseline.desired_plan().digest()
    );
    let extractor = absent_bundle_with(
        1,
        br#"{"path":"alpha.md","body":"Alpha links [[beta]]."}"#,
        br#"{"path":"alpha.md","kind":"file"}"#,
        br#"{"z":{"model":"v2"},"a":1}"#,
    );
    assert_ne!(
        extractor.prepared_input().digest(),
        baseline.prepared_input().digest()
    );
    assert_ne!(
        DataMappingV1::parse_json(br#"{"z":{"x":1},"a":2}"#)
            .unwrap()
            .digest(),
        DataMappingV1::parse_json(br#"{"a":2,"z":{"x":2}}"#)
            .unwrap()
            .digest()
    );
    let _ = CatalogMappingV1::parse_json(br#"{"custom":{"enabled":true}}"#).unwrap();
    let _ = GraphEdgeMappingV1::parse_json(br#"{"custom":{"enabled":true}}"#).unwrap();
    let _ = GraphNodeMappingV1::parse_json(br#"{"custom":{"enabled":true}}"#).unwrap();
    let _ = ExtractorConfigV1::parse_json(br#"["arbitrary",{"nested":true}]"#).unwrap();
}

#[test]
fn closed_objects_reject_unknown_and_duplicate_members_at_depth() {
    assert!(ManifestV1::parse_json(
        br#"{"entries":[],"format_version":1,"root_identity":"/r","unknown":1}"#
    )
    .is_err());
    assert!(ManifestV1::parse_json(
        br#"{"entries":[],"format_version":1,"format_version":1,"root_identity":"/r"}"#
    )
    .is_err());
    assert!(LogicalEdgeRowV1::parse_json(br#"{"src":"a","dst":"b","type":"t","weight":1,"confidence":1,"valid_at":0,"created_at":0,"detector":"d","schema_version":1,"src_file":"f","evidence":{"quote":"q","source":"f","offset":0,"unknown":1}}"#).is_err());
    assert!(LogicalNodeRowV1::parse_json(br#"{"source_index":"life-docs","logical_node_id":"doc-a","title":null,"preview":null,"path":"a","unknown":1}"#).is_err());
    let publication = include_bytes!("../testdata/review11-v1/publication.json");
    let mut unknown = b"{\"unknown\":1,".to_vec();
    unknown.extend_from_slice(&publication[1..]);
    assert!(CorpusPublicationV1::parse_closed_json(&unknown).is_err());
}

#[test]
fn integral_exponents_are_accepted_but_fractional_integral_fields_reject() {
    assert!(LogicalEdgeRowV1::parse_json(br#"{"src":"a","dst":"b","type":"t","weight":1,"confidence":1,"valid_at":1e2,"created_at":2e2,"detector":"d","schema_version":1e0,"src_file":"f","evidence":{"quote":"q","source":"f","offset":3e0}}"#).is_ok());
    assert!(LogicalEdgeRowV1::parse_json(br#"{"src":"a","dst":"b","type":"t","weight":1,"confidence":1,"valid_at":1e-1,"created_at":0,"detector":"d","schema_version":1,"src_file":"f","evidence":{"quote":"q","source":"f","offset":0}}"#).is_err());
}
