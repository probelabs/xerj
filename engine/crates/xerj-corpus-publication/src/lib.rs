//! Closed, I/O-free protocol kernel for corpus publication durable-begin records.
//!
//! This package deliberately contains no persistence or product integration.

mod begin;
mod canonical_json;
mod codec;
mod digest;
mod error;
mod identity;
mod logical_input;
mod manifest;
mod plan;
mod prepared;
mod projection;
mod publication;
mod replay;
mod scalar;

pub use begin::{
    DurableBeginBundleV1, PersistedDesiredPlanBytesV1, PersistedPreparedInputBytesV1,
    PersistedReplayArtifactBytesV1, PersistedSyncBeginBytesV1, SyncBeginJsonBytes, SyncBeginV1,
};
pub use digest::{
    CorpusIncarnationId, CorpusOwnerId, DesiredPlanDigest, ExpectedPublicationDigest,
    ExtractorConfigDigest, ManifestDigest, MappingDigest, PreparedInputDigest, PublicationDigest,
    ReplayArtifactDigest, ReplaySetDigest, SyncBeginDigest, TransactionId,
};
pub use error::{ProtocolError, ProtocolErrorKind};
pub use logical_input::{
    CatalogInputV1, CatalogMappingV1, CatalogWrapperV1, DataDocumentV1, DataMappingV1,
    DataRouteInputV1, ExtractorConfigV1, GraphEdgeMappingV1, GraphInputV1, GraphNodeMappingV1,
    LogicalEdgeRowV1, LogicalNodeRowV1, PrepareCorpusInputV1,
};
pub use manifest::{ManifestJsonBytes, ManifestV1};
pub use plan::{
    DesiredPlanBytes, DesiredPublicationPlanV1, MappingJsonBytes, MappingReservationV1,
    PlannedCorpusV1,
};
pub use prepared::{PreparedCorpusV1, PreparedInputBytes, PreparedInputV1, SequenceTransitionV1};
pub use publication::{
    CorpusPublicationJsonBytes, CorpusPublicationV1, ExpectedPublicationJsonBytes,
    ExpectedPublicationKind, ExpectedPublicationV1,
};
pub use replay::{ProjectionKind, ReplayArtifactBytes, ReplayArtifactKind, ReplayArtifactV1};
pub use scalar::{
    BrainName, CorpusIncarnationSeed, CorpusPrefix, DataSlug, DocumentId, ExtractorIdentity,
    Generation, LogicalEdgeId, LogicalIndexName, PhysicalDataName, ResourceKey, RootIdentity,
    Sequence, WrapperId,
};
