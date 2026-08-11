use std::{
    fmt::{Debug, Display},
    hash::Hash,
    str::FromStr,
};

#[path = "support/api_exports.rs"]
mod api_exports;

use xerj_corpus_publication::{
    BrainName, CatalogInputV1, CatalogMappingV1, CatalogWrapperV1, CorpusIncarnationId,
    CorpusIncarnationSeed, CorpusOwnerId, CorpusPrefix, CorpusPublicationJsonBytes,
    CorpusPublicationV1, DataDocumentV1, DataMappingV1, DataRouteInputV1, DataSlug,
    DesiredPlanBytes, DesiredPlanDigest, DesiredPublicationPlanV1, DocumentId,
    DurableBeginBundleV1, ExpectedPublicationDigest, ExpectedPublicationJsonBytes,
    ExpectedPublicationKind, ExpectedPublicationV1, ExtractorConfigDigest, ExtractorConfigV1,
    ExtractorIdentity, Generation, GraphEdgeMappingV1, GraphInputV1, GraphNodeMappingV1,
    LogicalEdgeId, LogicalEdgeRowV1, LogicalIndexName, LogicalNodeRowV1, ManifestDigest,
    ManifestJsonBytes, ManifestV1, MappingDigest, MappingJsonBytes, MappingReservationV1,
    PersistedDesiredPlanBytesV1, PersistedPreparedInputBytesV1, PersistedReplayArtifactBytesV1,
    PersistedSyncBeginBytesV1, PhysicalDataName, PlannedCorpusV1, PrepareCorpusInputV1,
    PreparedCorpusV1, PreparedInputBytes, PreparedInputDigest, PreparedInputV1, ProjectionKind,
    ProtocolError, ProtocolErrorKind, PublicationDigest, ReplayArtifactBytes, ReplayArtifactDigest,
    ReplayArtifactKind, ReplayArtifactV1, ReplaySetDigest, ResourceKey, RootIdentity, Sequence,
    SequenceTransitionV1, SyncBeginDigest, SyncBeginJsonBytes, SyncBeginV1, TransactionId,
    WrapperId,
};

const EXACT_CRATE_ROOT_EXPORTS: &[&str] = &[
    "BrainName",
    "CatalogInputV1",
    "CatalogMappingV1",
    "CatalogWrapperV1",
    "CorpusIncarnationId",
    "CorpusIncarnationSeed",
    "CorpusOwnerId",
    "CorpusPrefix",
    "CorpusPublicationJsonBytes",
    "CorpusPublicationV1",
    "DataDocumentV1",
    "DataMappingV1",
    "DataRouteInputV1",
    "DataSlug",
    "DesiredPlanBytes",
    "DesiredPlanDigest",
    "DesiredPublicationPlanV1",
    "DocumentId",
    "DurableBeginBundleV1",
    "ExpectedPublicationDigest",
    "ExpectedPublicationJsonBytes",
    "ExpectedPublicationKind",
    "ExpectedPublicationV1",
    "ExtractorConfigDigest",
    "ExtractorConfigV1",
    "ExtractorIdentity",
    "Generation",
    "GraphEdgeMappingV1",
    "GraphInputV1",
    "GraphNodeMappingV1",
    "LogicalEdgeId",
    "LogicalEdgeRowV1",
    "LogicalIndexName",
    "LogicalNodeRowV1",
    "ManifestDigest",
    "ManifestJsonBytes",
    "ManifestV1",
    "MappingDigest",
    "MappingJsonBytes",
    "MappingReservationV1",
    "PersistedDesiredPlanBytesV1",
    "PersistedPreparedInputBytesV1",
    "PersistedReplayArtifactBytesV1",
    "PersistedSyncBeginBytesV1",
    "PhysicalDataName",
    "PlannedCorpusV1",
    "PrepareCorpusInputV1",
    "PreparedCorpusV1",
    "PreparedInputBytes",
    "PreparedInputDigest",
    "PreparedInputV1",
    "ProjectionKind",
    "ProtocolError",
    "ProtocolErrorKind",
    "PublicationDigest",
    "ReplayArtifactBytes",
    "ReplayArtifactDigest",
    "ReplayArtifactKind",
    "ReplayArtifactV1",
    "ReplaySetDigest",
    "ResourceKey",
    "RootIdentity",
    "Sequence",
    "SequenceTransitionV1",
    "SyncBeginDigest",
    "SyncBeginJsonBytes",
    "SyncBeginV1",
    "TransactionId",
    "WrapperId",
];

fn copy_debug_eq_hash<T: Copy + Clone + Debug + Eq + Hash>() {}
fn string_traits<
    T: Clone
        + Debug
        + Eq
        + Ord
        + Hash
        + Display
        + FromStr<Err = ProtocolError>
        + TryFrom<String, Error = ProtocolError>,
>() {
}
fn digest_traits<T: Clone + Debug + Eq + Ord + Hash + Display + FromStr<Err = ProtocolError>>() {}

#[test]
fn crate_root_exports_match_the_exact_allowlist() {
    api_exports::audit_exact_crate_root_exports(
        include_str!("../src/lib.rs"),
        EXACT_CRATE_ROOT_EXPORTS,
    )
    .unwrap();
}

#[test]
fn export_audit_is_format_independent_and_fail_closed() {
    let compact = "pub use alpha::{A,beta::{B as C,self}};";
    let expanded = r#"
        pub use alpha::{
            A,
            beta::{
                B as C,
                self,
            },
        };
    "#;
    let expected = vec!["A".to_owned(), "C".to_owned(), "beta".to_owned()];
    assert_eq!(api_exports::crate_root_exports(compact).unwrap(), expected);
    assert_eq!(api_exports::crate_root_exports(expanded).unwrap(), expected);
    api_exports::audit_exact_crate_root_exports(compact, &["A", "C", "beta"]).unwrap();
    api_exports::audit_exact_crate_root_exports(expanded, &["A", "C", "beta"]).unwrap();

    assert_eq!(
        api_exports::audit_exact_crate_root_exports(
            "pub use stable::Known; pub struct Surprise;",
            &["Known"],
        )
        .unwrap_err(),
        "crate-root export ledger mismatch; unexpected=[\"Surprise\"]; missing=[]"
    );
    assert!(api_exports::crate_root_exports("pub use anything::*;").is_err());
    assert!(api_exports::crate_root_exports("make_public_items!();").is_err());
}

#[test]
fn exact_public_ledger_is_importable_and_has_required_positive_traits() {
    copy_debug_eq_hash::<ProtocolErrorKind>();
    copy_debug_eq_hash::<ProjectionKind>();
    copy_debug_eq_hash::<ReplayArtifactKind>();
    copy_debug_eq_hash::<ExpectedPublicationKind>();
    for _ in [
        std::any::TypeId::of::<CatalogInputV1>(),
        std::any::TypeId::of::<CatalogMappingV1>(),
        std::any::TypeId::of::<CatalogWrapperV1>(),
        std::any::TypeId::of::<CorpusIncarnationSeed>(),
        std::any::TypeId::of::<CorpusPublicationJsonBytes>(),
        std::any::TypeId::of::<CorpusPublicationV1>(),
        std::any::TypeId::of::<DataDocumentV1>(),
        std::any::TypeId::of::<DataMappingV1>(),
        std::any::TypeId::of::<DataRouteInputV1>(),
        std::any::TypeId::of::<DesiredPlanBytes>(),
        std::any::TypeId::of::<DesiredPublicationPlanV1>(),
        std::any::TypeId::of::<DurableBeginBundleV1>(),
        std::any::TypeId::of::<ExpectedPublicationJsonBytes>(),
        std::any::TypeId::of::<ExpectedPublicationV1>(),
        std::any::TypeId::of::<ExtractorConfigV1>(),
        std::any::TypeId::of::<GraphEdgeMappingV1>(),
        std::any::TypeId::of::<GraphInputV1>(),
        std::any::TypeId::of::<GraphNodeMappingV1>(),
        std::any::TypeId::of::<LogicalEdgeRowV1>(),
        std::any::TypeId::of::<LogicalNodeRowV1>(),
        std::any::TypeId::of::<ManifestJsonBytes>(),
        std::any::TypeId::of::<ManifestV1>(),
        std::any::TypeId::of::<MappingJsonBytes>(),
        std::any::TypeId::of::<MappingReservationV1>(),
        std::any::TypeId::of::<PersistedPreparedInputBytesV1>(),
        std::any::TypeId::of::<PersistedReplayArtifactBytesV1>(),
        std::any::TypeId::of::<PersistedDesiredPlanBytesV1>(),
        std::any::TypeId::of::<PersistedSyncBeginBytesV1>(),
        std::any::TypeId::of::<PlannedCorpusV1>(),
        std::any::TypeId::of::<PrepareCorpusInputV1>(),
        std::any::TypeId::of::<PreparedCorpusV1>(),
        std::any::TypeId::of::<PreparedInputBytes>(),
        std::any::TypeId::of::<PreparedInputV1>(),
        std::any::TypeId::of::<ReplayArtifactBytes>(),
        std::any::TypeId::of::<ReplayArtifactV1>(),
        std::any::TypeId::of::<SequenceTransitionV1>(),
        std::any::TypeId::of::<SyncBeginJsonBytes>(),
        std::any::TypeId::of::<SyncBeginV1>(),
    ] {}
    string_traits::<RootIdentity>();
    string_traits::<CorpusPrefix>();
    string_traits::<DataSlug>();
    string_traits::<LogicalIndexName>();
    string_traits::<BrainName>();
    string_traits::<ExtractorIdentity>();
    string_traits::<DocumentId>();
    string_traits::<WrapperId>();
    string_traits::<PhysicalDataName>();
    string_traits::<ResourceKey>();
    digest_traits::<CorpusOwnerId>();
    digest_traits::<CorpusIncarnationId>();
    digest_traits::<ManifestDigest>();
    digest_traits::<ExtractorConfigDigest>();
    digest_traits::<MappingDigest>();
    digest_traits::<PreparedInputDigest>();
    digest_traits::<TransactionId>();
    digest_traits::<ReplayArtifactDigest>();
    digest_traits::<ReplaySetDigest>();
    digest_traits::<DesiredPlanDigest>();
    digest_traits::<PublicationDigest>();
    digest_traits::<ExpectedPublicationDigest>();
    digest_traits::<SyncBeginDigest>();
    let _: Option<LogicalEdgeId> = None;
    let _: Option<Generation> = None;
    let _: Option<Sequence> = None;
}
