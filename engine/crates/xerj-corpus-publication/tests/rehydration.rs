mod support;

use std::str::FromStr;
use xerj_corpus_publication::*;

fn rehydrate(bundle: &DurableBeginBundleV1) -> Result<DurableBeginBundleV1, ProtocolError> {
    rehydrate_with_replay(bundle, persisted_replay(bundle))
}

fn persisted_replay(bundle: &DurableBeginBundleV1) -> Vec<PersistedReplayArtifactBytesV1> {
    bundle
        .replay_artifacts()
        .iter()
        .map(|artifact| {
            PersistedReplayArtifactBytesV1::from_journal(artifact.bytes().artifact_bytes().into())
        })
        .collect()
}

fn rehydrate_with_replay(
    bundle: &DurableBeginBundleV1,
    replay_artifacts: Vec<PersistedReplayArtifactBytesV1>,
) -> Result<DurableBeginBundleV1, ProtocolError> {
    DurableBeginBundleV1::rehydrate(
        PersistedPreparedInputBytesV1::from_journal(
            bundle
                .prepared_input()
                .canonical_preimage()
                .canonical_preimage()
                .into(),
        ),
        replay_artifacts,
        PersistedDesiredPlanBytesV1::from_journal(
            bundle
                .desired_plan()
                .canonical_preimage()
                .canonical_preimage()
                .into(),
        ),
        PersistedSyncBeginBytesV1::from_journal(
            bundle.sync_begin().canonical_json().canonical_json().into(),
        ),
    )
}

fn replay_sort_key(artifact: &ReplayArtifactV1) -> (String, &str, String, &str) {
    (
        artifact.projection_kind().to_string(),
        artifact.resource_key().as_protocol_str(),
        artifact.kind().to_string(),
        artifact.digest().as_rendered_str(),
    )
}

fn assert_bundle_eq(left: &DurableBeginBundleV1, right: &DurableBeginBundleV1) {
    assert_eq!(
        left.prepared_input()
            .canonical_preimage()
            .canonical_preimage(),
        right
            .prepared_input()
            .canonical_preimage()
            .canonical_preimage()
    );
    assert_eq!(
        left.prepared_input().digest(),
        right.prepared_input().digest()
    );
    assert_eq!(
        left.desired_plan()
            .canonical_preimage()
            .canonical_preimage(),
        right
            .desired_plan()
            .canonical_preimage()
            .canonical_preimage()
    );
    assert_eq!(left.desired_plan().digest(), right.desired_plan().digest());
    assert_eq!(
        left.sync_begin().canonical_json().canonical_json(),
        right.sync_begin().canonical_json().canonical_json()
    );
    assert_eq!(left.sync_begin().digest(), right.sync_begin().digest());
    assert_eq!(
        left.replay_artifacts().len(),
        right.replay_artifacts().len()
    );
    for (left, right) in left.replay_artifacts().iter().zip(right.replay_artifacts()) {
        assert_eq!(left.kind(), right.kind());
        assert_eq!(left.projection_kind(), right.projection_kind());
        assert_eq!(left.resource_key(), right.resource_key());
        assert_eq!(left.byte_length(), right.byte_length());
        assert_eq!(left.operation_count(), right.operation_count());
        assert_eq!(left.digest(), right.digest());
        assert_eq!(left.bytes(), right.bytes());
    }
}

#[test]
fn fresh_build_retains_the_original_controlled_byte_allocations() {
    let planned = support::absent_planned(7);
    let prepared_pointer = planned
        .prepared_input()
        .canonical_preimage()
        .canonical_preimage()
        .as_ptr();
    let plan_pointer = planned
        .desired_plan()
        .canonical_preimage()
        .canonical_preimage()
        .as_ptr();
    let replay_pointers: Vec<_> = planned
        .replay_artifacts()
        .iter()
        .map(|artifact| artifact.bytes().artifact_bytes().as_ptr())
        .collect();
    let owner = planned.desired_plan().owner().clone();

    let bundle =
        DurableBeginBundleV1::build(ExpectedPublicationV1::absent(owner), planned).unwrap();

    assert_eq!(
        prepared_pointer,
        bundle
            .prepared_input()
            .canonical_preimage()
            .canonical_preimage()
            .as_ptr()
    );
    assert_eq!(
        plan_pointer,
        bundle
            .desired_plan()
            .canonical_preimage()
            .canonical_preimage()
            .as_ptr()
    );
    assert_eq!(
        replay_pointers,
        bundle
            .replay_artifacts()
            .iter()
            .map(|artifact| artifact.bytes().artifact_bytes().as_ptr())
            .collect::<Vec<_>>()
    );
}

#[test]
fn fresh_and_rehydrated_bundles_are_byte_and_getter_identical() {
    let fresh = support::absent_bundle(7);
    assert!(fresh
        .replay_artifacts()
        .windows(2)
        .all(|pair| replay_sort_key(&pair[0]) < replay_sort_key(&pair[1])));
    assert_eq!(
        fresh.desired_plan().mapping_reservations().len(),
        fresh.replay_artifacts().len()
    );
    for (mapping, artifact) in fresh
        .desired_plan()
        .mapping_reservations()
        .iter()
        .zip(fresh.replay_artifacts())
    {
        assert_eq!(mapping.projection_kind(), artifact.projection_kind());
        assert_eq!(mapping.resource_key(), artifact.resource_key());
    }
    let recovered = rehydrate(&fresh).unwrap();
    assert_bundle_eq(&fresh, &recovered);
}

fn two_empty_data_routes() -> DurableBeginBundleV1 {
    let manifest =
        ManifestV1::parse_json(br#"{"entries":[],"format_version":1,"root_identity":"/r"}"#)
            .unwrap();
    let mapping = br#"{"properties":{"body":{"type":"text"}}}"#;
    let data = vec![
        DataRouteInputV1::new(
            DataSlug::from_str("docs").unwrap(),
            LogicalIndexName::from_str("life-docs").unwrap(),
            DataMappingV1::parse_json(mapping).unwrap(),
            Vec::new(),
        )
        .unwrap(),
        DataRouteInputV1::new(
            DataSlug::from_str("notes").unwrap(),
            LogicalIndexName::from_str("life-notes").unwrap(),
            DataMappingV1::parse_json(mapping).unwrap(),
            Vec::new(),
        )
        .unwrap(),
    ];
    let catalog = CatalogInputV1::new(
        CatalogMappingV1::parse_json(br#"{"properties":{}}"#).unwrap(),
        Vec::new(),
    )
    .unwrap();
    let graph = GraphInputV1::new(
        BrainName::from_str("life").unwrap(),
        ExtractorIdentity::from_str("extractor@1").unwrap(),
        ExtractorConfigV1::parse_json(br#"{}"#).unwrap(),
        GraphEdgeMappingV1::parse_json(br#"{"properties":{}}"#).unwrap(),
        GraphNodeMappingV1::parse_json(br#"{"properties":{}}"#).unwrap(),
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    let prepared = PreparedCorpusV1::prepare(
        PrepareCorpusInputV1::new(
            RootIdentity::from_str("/r").unwrap(),
            CorpusPrefix::from_str("life").unwrap(),
            CorpusIncarnationSeed::from_array(std::array::from_fn(|index| index as u8)),
            manifest,
            data,
            catalog,
            graph,
        )
        .unwrap(),
    )
    .unwrap();
    let planned = PlannedCorpusV1::plan(
        prepared,
        SequenceTransitionV1::new(Sequence::new(0), Sequence::new(1)).unwrap(),
        Generation::new(1),
    )
    .unwrap();
    let owner = planned.desired_plan().owner().clone();
    DurableBeginBundleV1::build(ExpectedPublicationV1::absent(owner), planned).unwrap()
}

#[test]
fn two_empty_same_kind_routes_are_positionally_rehydrated() {
    let fresh = two_empty_data_routes();
    let data_positions: Vec<_> = fresh
        .replay_artifacts()
        .iter()
        .enumerate()
        .filter(|(_, artifact)| artifact.kind() == ReplayArtifactKind::DataBulkNdjson)
        .map(|(index, artifact)| {
            assert!(artifact.bytes().artifact_bytes().is_empty());
            index
        })
        .collect();
    assert_eq!(data_positions.len(), 2);

    let mut replay: Vec<_> = fresh
        .replay_artifacts()
        .iter()
        .map(|artifact| {
            PersistedReplayArtifactBytesV1::from_journal(artifact.bytes().artifact_bytes().into())
        })
        .collect();
    replay.swap(data_positions[0], data_positions[1]);
    let recovered = DurableBeginBundleV1::rehydrate(
        PersistedPreparedInputBytesV1::from_journal(
            fresh
                .prepared_input()
                .canonical_preimage()
                .canonical_preimage()
                .into(),
        ),
        replay,
        PersistedDesiredPlanBytesV1::from_journal(
            fresh
                .desired_plan()
                .canonical_preimage()
                .canonical_preimage()
                .into(),
        ),
        PersistedSyncBeginBytesV1::from_journal(
            fresh.sync_begin().canonical_json().canonical_json().into(),
        ),
    )
    .unwrap();
    assert_bundle_eq(&fresh, &recovered);
}

#[test]
fn omitting_an_empty_artifact_is_rejected_before_positional_attachment() {
    let fresh = two_empty_data_routes();
    let empty_position = fresh
        .replay_artifacts()
        .iter()
        .position(|artifact| {
            artifact.projection_kind() == ProjectionKind::Data
                && artifact.bytes().artifact_bytes().is_empty()
        })
        .expect("empty data replay artifact");
    let mut replay = persisted_replay(&fresh);
    replay.remove(empty_position);

    let error = rehydrate_with_replay(&fresh, replay).unwrap_err();
    assert_eq!(error.kind(), ProtocolErrorKind::CrossFieldMismatch);
    assert_eq!(
        error.to_string(),
        "persisted replay artifact cardinality differs from desired-plan tuples"
    );
}

#[test]
fn adding_an_empty_artifact_is_rejected_before_positional_attachment() {
    let fresh = two_empty_data_routes();
    assert!(fresh
        .replay_artifacts()
        .iter()
        .any(|artifact| artifact.bytes().artifact_bytes().is_empty()));
    let mut replay = persisted_replay(&fresh);
    replay.push(PersistedReplayArtifactBytesV1::from_journal(
        Vec::new().into_boxed_slice(),
    ));

    let error = rehydrate_with_replay(&fresh, replay).unwrap_err();
    assert_eq!(error.kind(), ProtocolErrorKind::CrossFieldMismatch);
    assert_eq!(
        error.to_string(),
        "persisted replay artifact cardinality differs from desired-plan tuples"
    );
}

#[test]
fn distinct_replay_artifact_permutation_rejects() {
    let fresh = support::absent_bundle_with_data_route_count(2);
    let data_positions: Vec<_> = fresh
        .replay_artifacts()
        .iter()
        .enumerate()
        .filter(|(_, artifact)| artifact.kind() == ReplayArtifactKind::DataBulkNdjson)
        .map(|(index, _)| index)
        .collect();
    assert_eq!(data_positions.len(), 2);
    let mut replay: Vec<_> = fresh
        .replay_artifacts()
        .iter()
        .map(|artifact| {
            PersistedReplayArtifactBytesV1::from_journal(artifact.bytes().artifact_bytes().into())
        })
        .collect();
    replay.swap(data_positions[0], data_positions[1]);
    let error = DurableBeginBundleV1::rehydrate(
        PersistedPreparedInputBytesV1::from_journal(
            fresh
                .prepared_input()
                .canonical_preimage()
                .canonical_preimage()
                .into(),
        ),
        replay,
        PersistedDesiredPlanBytesV1::from_journal(
            fresh
                .desired_plan()
                .canonical_preimage()
                .canonical_preimage()
                .into(),
        ),
        PersistedSyncBeginBytesV1::from_journal(
            fresh.sync_begin().canonical_json().canonical_json().into(),
        ),
    )
    .unwrap_err();
    assert_eq!(error.kind(), ProtocolErrorKind::CrossFieldMismatch);
}

#[test]
fn standalone_plan_must_equal_the_embedded_plan_bytes() {
    let fresh = support::absent_bundle(1);
    let other = support::absent_bundle(7);
    let error = DurableBeginBundleV1::rehydrate(
        PersistedPreparedInputBytesV1::from_journal(
            fresh
                .prepared_input()
                .canonical_preimage()
                .canonical_preimage()
                .into(),
        ),
        fresh
            .replay_artifacts()
            .iter()
            .map(|artifact| {
                PersistedReplayArtifactBytesV1::from_journal(
                    artifact.bytes().artifact_bytes().into(),
                )
            })
            .collect(),
        PersistedDesiredPlanBytesV1::from_journal(
            other
                .desired_plan()
                .canonical_preimage()
                .canonical_preimage()
                .into(),
        ),
        PersistedSyncBeginBytesV1::from_journal(
            fresh.sync_begin().canonical_json().canonical_json().into(),
        ),
    )
    .unwrap_err();
    assert_eq!(error.kind(), ProtocolErrorKind::CrossFieldMismatch);
    assert_eq!(
        error.to_string(),
        "standalone desired-plan bytes differ from sync begin embedded plan bytes"
    );
}
