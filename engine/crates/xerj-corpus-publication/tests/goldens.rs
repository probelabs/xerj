mod support;

use support::{absent_bundle, present_bundle};
use xerj_corpus_publication::{
    CorpusPublicationV1, DesiredPublicationPlanV1, DurableBeginBundleV1, ExpectedPublicationV1,
    PreparedInputV1, ProjectionKind, ReplayArtifactKind, ReplayArtifactV1, SyncBeginV1,
};

fn data_artifact(bundle: &DurableBeginBundleV1) -> &ReplayArtifactV1 {
    bundle
        .replay_artifacts()
        .iter()
        .find(|artifact| artifact.projection_kind() == ProjectionKind::Data)
        .expect("data replay artifact")
}

#[test]
fn review11_cross_field_golden_is_byte_exact() {
    let bundle = absent_bundle(1);
    let mut owner_preimage = b"xerj-corpus-owner-v1\0".to_vec();
    support::reference_codec::s(&mut owner_preimage, b"/r");
    support::reference_codec::s(&mut owner_preimage, b"life");
    assert_eq!(
        support::reference_codec::rendered("xercpo1-sha256-", &owner_preimage),
        bundle.desired_plan().owner().as_rendered_str()
    );
    let prepared = bundle.prepared_input();
    assert_eq!(
        prepared.canonical_preimage().canonical_preimage().len(),
        1_303
    );
    assert_eq!(
        prepared.digest().as_rendered_str(),
        "xerpdi1-sha256-65371031701ba0fd68cb208c0519d53bb3425a5bf2ab702c72337e774c93c75b"
    );
    assert_eq!(
        bundle.desired_plan().transaction_id().as_rendered_str(),
        "xertx1-sha256-a59e758c686c26ec8a984bb2605b17b38c1df2f4a436559ed2f61f7aea43166b"
    );
    assert_eq!(
        bundle
            .desired_plan()
            .canonical_preimage()
            .canonical_preimage()
            .len(),
        6_291
    );
    assert_eq!(
        bundle.desired_plan().digest().as_rendered_str(),
        "xerdp1-sha256-d89848a7c515772c16724522b3b0e27aca4fd0a871fa7cd4ea46ab9967956963"
    );
    assert_eq!(
        bundle.desired_plan().replay_set_digest().as_rendered_str(),
        "xerrs1-sha256-a96471c5951c4984d5e3845eaf0df542d9953fc635dd28ad3e7f645321f30710"
    );
    assert_eq!(bundle.desired_plan().quota_charge(), 21_116);
    assert_eq!(
        bundle
            .replay_artifacts()
            .iter()
            .map(|v| (
                v.kind(),
                v.byte_length(),
                v.operation_count(),
                v.digest().as_rendered_str()
            ))
            .collect::<Vec<_>>(),
        vec![
            (
                ReplayArtifactKind::CatalogBulkNdjson,
                204,
                1,
                "xerra1-sha256-74beb339827428f5f9661ee401be51f824367a5a009f658fdc47c401229abbfd"
            ),
            (
                ReplayArtifactKind::DataBulkNdjson,
                585,
                2,
                "xerra1-sha256-8012fbc387970ed7580448cf38041ef6113e3ce31a09842f4bc75d9a6b09e353"
            ),
            (
                ReplayArtifactKind::GraphEdgeBulkNdjson,
                943,
                1,
                "xerra1-sha256-08c831caed4229f360b420e443b47949b25405595e82f43f27866d1197c1285a"
            ),
            (
                ReplayArtifactKind::GraphNodeBulkNdjson,
                1_343,
                2,
                "xerra1-sha256-2085188e86384ef8ddb0982829c9608167571a88427bce4f25746b33d8de50b9"
            ),
        ]
    );
    let parsed_prepared = PreparedInputV1::parse_canonical_preimage(
        prepared.canonical_preimage().canonical_preimage(),
    )
    .unwrap();
    assert_eq!(parsed_prepared.digest(), prepared.digest());
    let parsed_plan = DesiredPublicationPlanV1::parse_canonical_preimage(
        bundle
            .desired_plan()
            .canonical_preimage()
            .canonical_preimage(),
    )
    .unwrap();
    assert_eq!(parsed_plan.digest(), bundle.desired_plan().digest());
    let parsed_begin =
        SyncBeginV1::parse_closed_json(bundle.sync_begin().canonical_json().canonical_json())
            .unwrap();
    assert_eq!(
        parsed_begin.canonical_json(),
        bundle.sync_begin().canonical_json()
    );
    assert_eq!(parsed_begin.digest(), bundle.sync_begin().digest());
}

#[test]
fn complete_prior_publication_and_present_expectation_are_closed_and_exact() {
    let bytes = include_bytes!("../testdata/review11-v1/publication.json");
    let publication = CorpusPublicationV1::parse_closed_json(bytes).unwrap();
    assert_eq!(
        publication.canonical_json().canonical_json(),
        bytes.strip_suffix(b"\n").unwrap_or(bytes)
    );
    assert_eq!(
        publication.digest().as_rendered_str(),
        "xercp1-sha256-e670b2ad52ec1f50a26a4c792b6b4b2058ae641380bb25e2a0102f7648694af5"
    );
    let expected = ExpectedPublicationV1::present(publication).unwrap();
    let reparsed =
        ExpectedPublicationV1::parse_closed_json(expected.canonical_json().canonical_json())
            .unwrap();
    assert_eq!(reparsed.digest(), expected.digest());
    assert_eq!(reparsed.canonical_json(), expected.canonical_json());
}

#[test]
fn complete_present_sync_begin_parses_and_reencodes_exactly() {
    let bundle = present_bundle();
    let parsed =
        SyncBeginV1::parse_closed_json(bundle.sync_begin().canonical_json().canonical_json())
            .unwrap();
    assert_eq!(
        parsed.canonical_json(),
        bundle.sync_begin().canonical_json()
    );
    assert_eq!(
        parsed.expected_publication().kind(),
        xerj_corpus_publication::ExpectedPublicationKind::Present
    );
}

#[test]
fn generation_is_typed_and_independent_of_sequence() {
    let one = absent_bundle(1);
    let seven = absent_bundle(7);
    assert_eq!(
        one.desired_plan().transaction_id(),
        seven.desired_plan().transaction_id()
    );
    assert_ne!(one.desired_plan().digest(), seven.desired_plan().digest());
    assert_eq!(seven.desired_plan().generation().get(), 7);
    assert_ne!(data_artifact(&one).bytes(), data_artifact(&seven).bytes());
}
