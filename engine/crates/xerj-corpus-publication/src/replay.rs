use crate::{
    canonical_json::{self, JsonValue},
    codec::Encoder,
    digest::{
        CatalogIdDigest, CatalogWrapperDigest, CorpusIncarnationId, CorpusOwnerId,
        DataContentDigest, DataIdDigest, EdgePhysicalId, EdgePhysicalIdSetDigest, GraphToken,
        LogicalEdgeSetDigest, LogicalNodeSetDigest, NodePhysicalId, NodePhysicalIdSetDigest,
        ProducerId, ReplayArtifactDigest, TransactionId,
    },
    error::{error, ProtocolError, ProtocolErrorKind, Result},
    logical_input::{LogicalEdgeRowV1, LogicalNodeRowV1},
    scalar::{DocumentId, Generation, ResourceKey, WrapperId},
};
use std::{fmt, str::FromStr};

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum ProjectionKind {
    Data,
    Catalog,
    GraphEdge,
    GraphNode,
}
impl ProjectionKind {
    pub(crate) fn protocol_str(self) -> &'static str {
        match self {
            Self::Data => "data",
            Self::Catalog => "catalog",
            Self::GraphEdge => "graph-edge",
            Self::GraphNode => "graph-node",
        }
    }
}
impl fmt::Display for ProjectionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.protocol_str())
    }
}
impl FromStr for ProjectionKind {
    type Err = ProtocolError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "data" => Ok(Self::Data),
            "catalog" => Ok(Self::Catalog),
            "graph-edge" => Ok(Self::GraphEdge),
            "graph-node" => Ok(Self::GraphNode),
            _ => Err(error(
                ProtocolErrorKind::InvalidScalar,
                "invalid projection kind",
            )),
        }
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum ReplayArtifactKind {
    DataBulkNdjson,
    CatalogBulkNdjson,
    GraphEdgeBulkNdjson,
    GraphNodeBulkNdjson,
}
impl ReplayArtifactKind {
    pub(crate) fn protocol_str(self) -> &'static str {
        match self {
            Self::DataBulkNdjson => "data-bulk-ndjson",
            Self::CatalogBulkNdjson => "catalog-bulk-ndjson",
            Self::GraphEdgeBulkNdjson => "graph-edge-bulk-ndjson",
            Self::GraphNodeBulkNdjson => "graph-node-bulk-ndjson",
        }
    }
}
impl fmt::Display for ReplayArtifactKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.protocol_str())
    }
}
impl FromStr for ReplayArtifactKind {
    type Err = ProtocolError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "data-bulk-ndjson" => Ok(Self::DataBulkNdjson),
            "catalog-bulk-ndjson" => Ok(Self::CatalogBulkNdjson),
            "graph-edge-bulk-ndjson" => Ok(Self::GraphEdgeBulkNdjson),
            "graph-node-bulk-ndjson" => Ok(Self::GraphNodeBulkNdjson),
            _ => Err(error(
                ProtocolErrorKind::InvalidScalar,
                "invalid replay artifact kind",
            )),
        }
    }
}

pub struct ReplayArtifactBytes(Box<[u8]>);
impl ReplayArtifactBytes {
    pub fn artifact_bytes(&self) -> &[u8] {
        &self.0
    }
}
impl PartialEq for ReplayArtifactBytes {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl Eq for ReplayArtifactBytes {}
impl fmt::Debug for ReplayArtifactBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReplayArtifactBytes")
            .field("len", &self.0.len())
            .finish()
    }
}

pub struct ReplayArtifactV1 {
    pub(crate) kind: ReplayArtifactKind,
    pub(crate) projection_kind: ProjectionKind,
    pub(crate) resource_key: ResourceKey,
    pub(crate) operation_count: u64,
    pub(crate) digest: ReplayArtifactDigest,
    pub(crate) bytes: ReplayArtifactBytes,
}
impl ReplayArtifactV1 {
    pub fn kind(&self) -> ReplayArtifactKind {
        self.kind
    }
    pub fn projection_kind(&self) -> ProjectionKind {
        self.projection_kind
    }
    pub fn resource_key(&self) -> &ResourceKey {
        &self.resource_key
    }
    pub fn byte_length(&self) -> u64 {
        self.bytes.0.len() as u64
    }
    pub fn operation_count(&self) -> u64 {
        self.operation_count
    }
    pub fn digest(&self) -> &ReplayArtifactDigest {
        &self.digest
    }
    pub fn bytes(&self) -> &ReplayArtifactBytes {
        &self.bytes
    }
    pub(crate) fn new(
        kind: ReplayArtifactKind,
        projection_kind: ProjectionKind,
        resource_key: ResourceKey,
        operation_count: u64,
        bytes: Vec<u8>,
    ) -> Self {
        let digest = crate::prepared::artifact_digest(kind.protocol_str(), &bytes);
        Self {
            kind,
            projection_kind,
            resource_key,
            operation_count,
            digest,
            bytes: ReplayArtifactBytes(bytes.into_boxed_slice()),
        }
    }

    pub(crate) fn from_persisted_tuple(
        kind: ReplayArtifactKind,
        projection_kind: ProjectionKind,
        resource_key: ResourceKey,
        operation_count: u64,
        digest: ReplayArtifactDigest,
        bytes: Box<[u8]>,
    ) -> Self {
        Self {
            kind,
            projection_kind,
            resource_key,
            operation_count,
            digest,
            bytes: ReplayArtifactBytes(bytes),
        }
    }
}
impl fmt::Debug for ReplayArtifactV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReplayArtifactV1")
            .field("kind", &self.kind)
            .field("projection_kind", &self.projection_kind)
            .field("resource_key", &self.resource_key)
            .field("operation_count", &self.operation_count)
            .field("digest", &self.digest)
            .field("bytes", &self.bytes)
            .finish()
    }
}

/// Metadata already decoded from one desired-plan replay tuple and its
/// matching projection. The persisted artifact validator intentionally takes
/// this by position: it does not search or reorder artifacts by their bytes.
#[allow(dead_code)] // consumed by the revision-2 bundle validator integration
pub(crate) struct ReplayTupleExpectationV1<'a> {
    pub(crate) kind: ReplayArtifactKind,
    pub(crate) projection_kind: ProjectionKind,
    pub(crate) resource_key: &'a ResourceKey,
    pub(crate) target: &'a str,
    pub(crate) byte_length: u64,
    pub(crate) operation_count: u64,
    pub(crate) digest: &'a ReplayArtifactDigest,
    pub(crate) owner: &'a CorpusOwnerId,
    pub(crate) corpus_incarnation: &'a CorpusIncarnationId,
    pub(crate) generation: Generation,
    pub(crate) transaction: &'a TransactionId,
    pub(crate) graph_producer: &'a ProducerId,
}

#[derive(Debug)]
pub(crate) enum ReplayEvidenceV1 {
    Data {
        ids: DataIdDigest,
        content: DataContentDigest,
        prepared_payload: ReplayArtifactDigest,
    },
    Catalog {
        ids: CatalogIdDigest,
        content: CatalogWrapperDigest,
        prepared_payload: ReplayArtifactDigest,
    },
    GraphEdge {
        logical: LogicalEdgeSetDigest,
        physical_ids: EdgePhysicalIdSetDigest,
    },
    GraphNode {
        logical: LogicalNodeSetDigest,
        physical_ids: NodePhysicalIdSetDigest,
    },
}

/// Strictly validate one target-bearing replay artifact against the tuple at
/// the same canonical position in the desired plan.
#[cfg(test)]
pub(crate) fn validate_replay_artifact_v1(
    expected: ReplayTupleExpectationV1<'_>,
    bytes: Box<[u8]>,
) -> Result<ReplayArtifactV1> {
    let evidence = validate_replay_artifact_bytes_v1(&expected, &bytes)?;
    let operation_count = expected.operation_count;
    finish_validated_artifact(expected, bytes, operation_count, evidence)
}

/// Validate replay bytes without taking ownership. Fresh bundle validation
/// uses this path so the controlled artifact allocation survives completion.
pub(crate) fn validate_replay_artifact_bytes_v1(
    expected: &ReplayTupleExpectationV1<'_>,
    bytes: &[u8],
) -> Result<ReplayEvidenceV1> {
    validate_kind_projection_resource(expected)?;

    let actual_length = u64::try_from(bytes.len()).map_err(|_| {
        error(
            ProtocolErrorKind::BoundsExceeded,
            "replay artifact byte length exceeds u64",
        )
    })?;
    if actual_length != expected.byte_length {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "replay artifact byte length differs from desired-plan tuple",
        ));
    }

    if bytes.is_empty() {
        if expected.byte_length != 0 || expected.operation_count != 0 {
            return Err(error(
                ProtocolErrorKind::CrossFieldMismatch,
                "empty replay artifact requires zero bytes and zero operations",
            ));
        }
        let evidence = empty_evidence(expected.kind);
        let digest = crate::prepared::artifact_digest(expected.kind.protocol_str(), bytes);
        if &digest != expected.digest {
            return Err(error(
                ProtocolErrorKind::CrossFieldMismatch,
                "replay artifact digest differs from desired-plan tuple",
            ));
        }
        return Ok(evidence);
    }

    if bytes.contains(&b'\r') {
        return Err(error(
            ProtocolErrorKind::NonCanonicalEncoding,
            "replay artifact contains CR",
        ));
    }
    if !bytes.ends_with(b"\n") {
        return Err(error(
            ProtocolErrorKind::NonCanonicalEncoding,
            "nonempty replay artifact is missing its final LF",
        ));
    }

    let body = &bytes[..bytes.len() - 1];
    let lines: Vec<&[u8]> = body.split(|byte| *byte == b'\n').collect();
    if lines.iter().any(|line| line.is_empty()) {
        return Err(error(
            ProtocolErrorKind::NonCanonicalEncoding,
            "replay artifact contains a blank line",
        ));
    }
    if !lines.len().is_multiple_of(2) {
        return Err(error(
            ProtocolErrorKind::InvalidJson,
            "replay artifact must contain complete action/source pairs",
        ));
    }

    let operation_count = u64::try_from(lines.len() / 2).map_err(|_| {
        error(
            ProtocolErrorKind::BoundsExceeded,
            "replay artifact operation count exceeds u64",
        )
    })?;
    if operation_count != expected.operation_count {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "replay artifact operation count differs from desired-plan tuple",
        ));
    }

    let mut previous_key: Option<RowOrderKey> = None;
    let mut rows = Vec::with_capacity(lines.len() / 2);
    for pair in lines.chunks_exact(2) {
        let action = parse_canonical_line(pair[0], "replay action")?;
        let source = parse_canonical_line(pair[1], "replay source")?;
        let (id, generation) = parse_action(&action, expected)?;
        let row = validate_source(expected, id, generation, &source)?;
        let key = row.order_key();
        if let Some(previous) = previous_key.as_ref() {
            if key.duplicate_identity(previous) {
                return Err(error(
                    ProtocolErrorKind::DuplicateTuple,
                    "replay artifact contains a duplicate logical row identity",
                ));
            }
            if key <= *previous {
                return Err(error(
                    ProtocolErrorKind::NonCanonicalEncoding,
                    "replay artifact rows are not in normative logical order",
                ));
            }
        }
        previous_key = Some(key);
        rows.push(row);
    }

    let evidence = finish_evidence(expected.kind, rows)?;
    let digest = crate::prepared::artifact_digest(expected.kind.protocol_str(), bytes);
    if &digest != expected.digest {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "replay artifact digest differs from desired-plan tuple",
        ));
    }
    Ok(evidence)
}

#[cfg(test)]
fn finish_validated_artifact(
    expected: ReplayTupleExpectationV1<'_>,
    bytes: Box<[u8]>,
    operation_count: u64,
    evidence: ReplayEvidenceV1,
) -> Result<ReplayArtifactV1> {
    let digest = crate::prepared::artifact_digest(expected.kind.protocol_str(), &bytes);
    if &digest != expected.digest {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "replay artifact digest differs from desired-plan tuple",
        ));
    }
    let _ = evidence;
    Ok(ReplayArtifactV1 {
        kind: expected.kind,
        projection_kind: expected.projection_kind,
        resource_key: expected.resource_key.clone(),
        operation_count,
        digest,
        bytes: ReplayArtifactBytes(bytes),
    })
}

fn parse_canonical_line(line: &[u8], field: &str) -> Result<JsonValue> {
    let value = canonical_json::parse(line, field)?;
    if canonical_json::canonicalize(&value) != line {
        return Err(error(
            ProtocolErrorKind::NonCanonicalEncoding,
            format_args!("{field} is not RFC 8785 canonical JSON"),
        ));
    }
    Ok(value)
}

fn validate_kind_projection_resource(expected: &ReplayTupleExpectationV1<'_>) -> Result<()> {
    let compatible = matches!(
        (expected.kind, expected.projection_kind),
        (ReplayArtifactKind::DataBulkNdjson, ProjectionKind::Data)
            | (
                ReplayArtifactKind::CatalogBulkNdjson,
                ProjectionKind::Catalog
            )
            | (
                ReplayArtifactKind::GraphEdgeBulkNdjson,
                ProjectionKind::GraphEdge
            )
            | (
                ReplayArtifactKind::GraphNodeBulkNdjson,
                ProjectionKind::GraphNode
            )
    );
    if !compatible {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "replay artifact kind does not match projection kind",
        ));
    }

    let resource = expected.resource_key.as_protocol_str();
    let derived_target = match expected.kind {
        ReplayArtifactKind::DataBulkNdjson => resource.strip_prefix("data/"),
        ReplayArtifactKind::CatalogBulkNdjson => resource
            .strip_prefix("catalog/")
            .and_then(|rest| rest.rsplit_once('/').map(|(target, _)| target)),
        ReplayArtifactKind::GraphEdgeBulkNdjson => resource
            .strip_prefix("graph-edge/")
            .and_then(|rest| rest.rsplit_once('/').map(|(target, _)| target)),
        ReplayArtifactKind::GraphNodeBulkNdjson => resource
            .strip_prefix("graph-node/")
            .and_then(|rest| rest.rsplit_once('/').map(|(target, _)| target)),
    };
    if derived_target != Some(expected.target) {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "replay target does not match its resource key",
        ));
    }
    Ok(())
}

fn parse_action<'a>(
    action: &'a JsonValue,
    expected: &ReplayTupleExpectationV1<'_>,
) -> Result<(&'a str, Option<&'a str>)> {
    let outer = canonical_json::closed(action, "replay action", &["index"])?;
    let names: &[&str] = match expected.kind {
        ReplayArtifactKind::CatalogBulkNdjson => &["_id", "_index", "generation"],
        _ => &["_id", "_index"],
    };
    let metadata = canonical_json::closed(outer[0], "replay action.index", names)?;
    let id = canonical_json::string(metadata[0], "replay action.index._id")?;
    let target = canonical_json::string(metadata[1], "replay action.index._index")?;
    if target != expected.target {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "replay action target differs from desired projection",
        ));
    }
    let generation = if metadata.len() == 3 {
        Some(canonical_json::string(
            metadata[2],
            "replay action.index.generation",
        )?)
    } else {
        None
    };
    Ok((id, generation))
}

#[derive(Eq, Ord, PartialEq, PartialOrd)]
enum RowOrderKey {
    Data(Box<str>),
    Catalog(Box<str>),
    GraphEdge(Box<str>, Box<[u8]>),
    GraphNode(Box<str>, Box<str>),
}

enum ValidatedRow {
    Data {
        id: Box<str>,
        source: Box<[u8]>,
    },
    Catalog {
        id: Box<str>,
        source: Box<[u8]>,
    },
    GraphEdge {
        logical_id: Box<str>,
        logical_source: Box<[u8]>,
        physical_id: Box<str>,
    },
    GraphNode {
        source_index: Box<str>,
        logical_node_id: Box<str>,
        logical_source: Box<[u8]>,
        physical_id: Box<str>,
    },
}

impl ValidatedRow {
    fn order_key(&self) -> RowOrderKey {
        match self {
            Self::Data { id, .. } => RowOrderKey::Data(id.clone()),
            Self::Catalog { id, .. } => RowOrderKey::Catalog(id.clone()),
            Self::GraphEdge {
                logical_id,
                logical_source,
                ..
            } => RowOrderKey::GraphEdge(logical_id.clone(), logical_source.clone()),
            Self::GraphNode {
                source_index,
                logical_node_id,
                ..
            } => RowOrderKey::GraphNode(source_index.clone(), logical_node_id.clone()),
        }
    }
}

impl RowOrderKey {
    fn duplicate_identity(&self, previous: &Self) -> bool {
        match (self, previous) {
            (Self::Data(left), Self::Data(right)) | (Self::Catalog(left), Self::Catalog(right)) => {
                left == right
            }
            (Self::GraphEdge(left, _), Self::GraphEdge(right, _)) => left == right,
            (Self::GraphNode(ls, li), Self::GraphNode(rs, ri)) => ls == rs && li == ri,
            _ => false,
        }
    }
}

fn empty_evidence(kind: ReplayArtifactKind) -> ReplayEvidenceV1 {
    finish_evidence(kind, Vec::new()).expect("empty evidence has no fallible row conversion")
}

fn finish_evidence(kind: ReplayArtifactKind, rows: Vec<ValidatedRow>) -> Result<ReplayEvidenceV1> {
    match kind {
        ReplayArtifactKind::DataBulkNdjson => {
            let mut ids = Encoder::domain(b"xerj-id-set-v1\0");
            let mut content = Encoder::domain(b"xerj-data-content-v1\0");
            ids.array_len(rows.len());
            content.array_len(rows.len());
            let mut payload = Vec::new();
            for row in rows {
                let ValidatedRow::Data { id, source } = row else {
                    return Err(error(
                        ProtocolErrorKind::CrossFieldMismatch,
                        "data replay contains a row of another artifact kind",
                    ));
                };
                ids.string(&id);
                content.string(&id);
                content.bytes(&source);
                payload.extend_from_slice(
                    format!("{{\"id\":{}}}\n", canonical_json::json_string(&id)).as_bytes(),
                );
                payload.extend_from_slice(&source);
                payload.push(b'\n');
            }
            Ok(ReplayEvidenceV1::Data {
                ids: DataIdDigest::from_preimage(&ids.finish()),
                content: DataContentDigest::from_preimage(&content.finish()),
                prepared_payload: crate::prepared::artifact_digest("prepared-data-rows", &payload),
            })
        }
        ReplayArtifactKind::CatalogBulkNdjson => {
            let mut ids = Encoder::domain(b"xerj-catalog-id-set-v1\0");
            let mut content = Encoder::domain(b"xerj-catalog-wrapper-set-v1\0");
            ids.array_len(rows.len());
            content.array_len(rows.len());
            let mut payload = Vec::new();
            for row in rows {
                let ValidatedRow::Catalog { id, source } = row else {
                    return Err(error(
                        ProtocolErrorKind::CrossFieldMismatch,
                        "catalog replay contains a row of another artifact kind",
                    ));
                };
                ids.string(&id);
                content.string(&id);
                content.bytes(&source);
                payload.extend_from_slice(
                    format!("{{\"id\":{}}}\n", canonical_json::json_string(&id)).as_bytes(),
                );
                payload.extend_from_slice(&source);
                payload.push(b'\n');
            }
            Ok(ReplayEvidenceV1::Catalog {
                ids: CatalogIdDigest::from_preimage(&ids.finish()),
                content: CatalogWrapperDigest::from_preimage(&content.finish()),
                prepared_payload: crate::prepared::artifact_digest(
                    "prepared-catalog-rows",
                    &payload,
                ),
            })
        }
        ReplayArtifactKind::GraphEdgeBulkNdjson => {
            let mut logical = Encoder::domain(b"xerj-graph-logical-edges-v1\0");
            logical.array_len(rows.len());
            let mut physical_ids = Vec::with_capacity(rows.len());
            for row in rows {
                let ValidatedRow::GraphEdge {
                    logical_id,
                    logical_source,
                    physical_id,
                } = row
                else {
                    return Err(error(
                        ProtocolErrorKind::CrossFieldMismatch,
                        "graph-edge replay contains a row of another artifact kind",
                    ));
                };
                logical.string(&logical_id);
                logical.bytes(&logical_source);
                physical_ids.push(physical_id);
            }
            physical_ids.sort_unstable();
            let mut physical = Encoder::domain(b"xerj-graph-edge-physical-ids-v1\0");
            physical.array_len(physical_ids.len());
            for id in physical_ids {
                physical.string(&id);
            }
            Ok(ReplayEvidenceV1::GraphEdge {
                logical: LogicalEdgeSetDigest::from_preimage(&logical.finish()),
                physical_ids: EdgePhysicalIdSetDigest::from_preimage(&physical.finish()),
            })
        }
        ReplayArtifactKind::GraphNodeBulkNdjson => {
            let mut logical = Encoder::domain(b"xerj-graph-logical-nodes-v1\0");
            logical.array_len(rows.len());
            let mut physical_ids = Vec::with_capacity(rows.len());
            for row in rows {
                let ValidatedRow::GraphNode {
                    source_index,
                    logical_node_id,
                    logical_source,
                    physical_id,
                } = row
                else {
                    return Err(error(
                        ProtocolErrorKind::CrossFieldMismatch,
                        "graph-node replay contains a row of another artifact kind",
                    ));
                };
                logical.string(&source_index);
                logical.string(&logical_node_id);
                logical.bytes(&logical_source);
                physical_ids.push(physical_id);
            }
            physical_ids.sort_unstable();
            let mut physical = Encoder::domain(b"xerj-graph-node-physical-ids-v1\0");
            physical.array_len(physical_ids.len());
            for id in physical_ids {
                physical.string(&id);
            }
            Ok(ReplayEvidenceV1::GraphNode {
                logical: LogicalNodeSetDigest::from_preimage(&logical.finish()),
                physical_ids: NodePhysicalIdSetDigest::from_preimage(&physical.finish()),
            })
        }
    }
}

fn validate_source(
    expected: &ReplayTupleExpectationV1<'_>,
    action_id: &str,
    action_generation: Option<&str>,
    source: &JsonValue,
) -> Result<ValidatedRow> {
    match expected.kind {
        ReplayArtifactKind::DataBulkNdjson => {
            let id = DocumentId::from_str(action_id)?;
            canonical_json::object(source, "replay data source")?;
            Ok(ValidatedRow::Data {
                id: id.as_protocol_str().into(),
                source: canonical_json::canonicalize(source).into_boxed_slice(),
            })
        }
        ReplayArtifactKind::CatalogBulkNdjson => {
            let id = WrapperId::from_str(action_id)?;
            canonical_json::object(source, "replay catalog source")?;
            let expected_generation = expected
                .resource_key
                .as_protocol_str()
                .rsplit_once('/')
                .map(|(_, generation)| generation)
                .ok_or_else(|| {
                    error(
                        ProtocolErrorKind::CrossFieldMismatch,
                        "catalog resource omits generation identity",
                    )
                })?;
            if action_generation != Some(expected_generation) {
                return Err(error(
                    ProtocolErrorKind::CrossFieldMismatch,
                    "catalog action generation differs from its resource key",
                ));
            }
            Ok(ValidatedRow::Catalog {
                id: id.as_protocol_str().into(),
                source: canonical_json::canonicalize(source).into_boxed_slice(),
            })
        }
        ReplayArtifactKind::GraphEdgeBulkNdjson => {
            validate_graph_edge_source(expected, action_id, source)
        }
        ReplayArtifactKind::GraphNodeBulkNdjson => {
            validate_graph_node_source(expected, action_id, source)
        }
    }
}

fn validate_graph_edge_source(
    expected: &ReplayTupleExpectationV1<'_>,
    action_id: &str,
    source: &JsonValue,
) -> Result<ValidatedRow> {
    let action_id = EdgePhysicalId::from_str(action_id)?;
    let generated = graph_generated_fields(
        source,
        &[
            "corpus_incarnation",
            "edge_scope",
            "graph_generation",
            "graph_owner",
            "graph_producer",
            "logical_edge_id",
            "physical_id",
            "tx_id",
        ],
        "replay graph-edge source",
    )?;
    let incarnation: CorpusIncarnationId =
        canonical_json::string(generated[0], "replay graph-edge source.corpus_incarnation")?
            .parse()?;
    if canonical_json::string(generated[1], "replay graph-edge source.edge_scope")? != "generated" {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "graph-edge edge_scope must be generated",
        ));
    }
    let generation = Generation::new(canonical_json::u64(
        generated[2],
        "replay graph-edge source.graph_generation",
    )?);
    let owner: CorpusOwnerId =
        canonical_json::string(generated[3], "replay graph-edge source.graph_owner")?.parse()?;
    let producer: ProducerId =
        canonical_json::string(generated[4], "replay graph-edge source.graph_producer")?.parse()?;
    let declared_logical_id =
        canonical_json::string(generated[5], "replay graph-edge source.logical_edge_id")?;
    let physical_id = canonical_json::string(generated[6], "replay graph-edge source.physical_id")?;
    let transaction: TransactionId =
        canonical_json::string(generated[7], "replay graph-edge source.tx_id")?.parse()?;
    if &incarnation != expected.corpus_incarnation
        || generation != expected.generation
        || &owner != expected.owner
        || &producer != expected.graph_producer
        || &transaction != expected.transaction
    {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "graph-edge authority fields differ from desired plan",
        ));
    }

    let logical = graph_logical_source(
        source,
        &[
            "corpus_incarnation",
            "edge_scope",
            "graph_generation",
            "graph_owner",
            "graph_producer",
            "logical_edge_id",
            "physical_id",
            "tx_id",
        ],
    );
    let logical_bytes = canonical_json::canonicalize(&logical);
    let logical = LogicalEdgeRowV1::parse_json(&logical_bytes)?;
    if declared_logical_id != logical.logical_id.as_lower_hex() {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "graph-edge logical_edge_id differs from logical row",
        ));
    }
    let token = resource_graph_token(expected.resource_key, "graph-edge")?;
    let recomputed = crate::identity::edge_physical_id(
        &owner,
        &incarnation,
        generation,
        &token,
        logical.logical_id.as_lower_hex(),
    );
    if recomputed != action_id || physical_id != action_id.as_rendered_str() {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "graph-edge physical identity does not match action and generated fields",
        ));
    }
    Ok(ValidatedRow::GraphEdge {
        logical_id: logical.logical_id.as_lower_hex().into(),
        logical_source: logical.canonical,
        physical_id: action_id.as_rendered_str().into(),
    })
}

fn validate_graph_node_source(
    expected: &ReplayTupleExpectationV1<'_>,
    action_id: &str,
    source: &JsonValue,
) -> Result<ValidatedRow> {
    let action_id = NodePhysicalId::from_str(action_id)?;
    let generated = graph_generated_fields(
        source,
        &[
            "corpus_incarnation",
            "doc_kind",
            "graph_generation",
            "graph_owner",
            "physical_id",
            "tx_id",
        ],
        "replay graph-node source",
    )?;
    let incarnation: CorpusIncarnationId =
        canonical_json::string(generated[0], "replay graph-node source.corpus_incarnation")?
            .parse()?;
    if canonical_json::string(generated[1], "replay graph-node source.doc_kind")? != "generated" {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "graph-node doc_kind must be generated",
        ));
    }
    let generation = Generation::new(canonical_json::u64(
        generated[2],
        "replay graph-node source.graph_generation",
    )?);
    let owner: CorpusOwnerId =
        canonical_json::string(generated[3], "replay graph-node source.graph_owner")?.parse()?;
    let physical_id = canonical_json::string(generated[4], "replay graph-node source.physical_id")?;
    let transaction: TransactionId =
        canonical_json::string(generated[5], "replay graph-node source.tx_id")?.parse()?;
    if &incarnation != expected.corpus_incarnation
        || generation != expected.generation
        || &owner != expected.owner
        || &transaction != expected.transaction
    {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "graph-node authority fields differ from desired plan",
        ));
    }

    let logical = graph_logical_source(
        source,
        &[
            "corpus_incarnation",
            "doc_kind",
            "graph_generation",
            "graph_owner",
            "physical_id",
            "tx_id",
        ],
    );
    let logical_bytes = canonical_json::canonicalize(&logical);
    let logical = LogicalNodeRowV1::parse_json(&logical_bytes)?;
    let token = resource_graph_token(expected.resource_key, "graph-node")?;
    let recomputed = crate::identity::node_physical_id(
        &owner,
        &incarnation,
        generation,
        &token,
        &logical.source_index,
        &logical.logical_node_id,
    );
    if recomputed != action_id || physical_id != action_id.as_rendered_str() {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "graph-node physical identity does not match action and generated fields",
        ));
    }
    Ok(ValidatedRow::GraphNode {
        source_index: logical.source_index,
        logical_node_id: logical.logical_node_id,
        logical_source: logical.canonical,
        physical_id: action_id.as_rendered_str().into(),
    })
}

fn graph_generated_fields<'a>(
    source: &'a JsonValue,
    names: &[&str],
    field: &str,
) -> Result<Vec<&'a JsonValue>> {
    let members = canonical_json::object(source, field)?;
    for name in names {
        if !members.iter().any(|(key, _)| key == name) {
            return Err(error(
                ProtocolErrorKind::InvalidJson,
                format_args!("{field}.{name} is missing"),
            ));
        }
    }
    names
        .iter()
        .map(|name| canonical_json::member(source, field, name))
        .collect()
}

fn graph_logical_source(source: &JsonValue, generated_names: &[&str]) -> JsonValue {
    let members = match source {
        JsonValue::Object(members) => members,
        _ => unreachable!("graph_generated_fields validates object shape first"),
    };
    JsonValue::Object(
        members
            .iter()
            .filter(|(name, _)| !generated_names.contains(&name.as_str()))
            .cloned()
            .collect(),
    )
}

fn resource_graph_token(resource: &ResourceKey, prefix: &str) -> Result<GraphToken> {
    let resource = resource
        .as_protocol_str()
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('/'))
        .and_then(|rest| rest.rsplit_once('/').map(|(_, token)| token))
        .ok_or_else(|| {
            error(
                ProtocolErrorKind::CrossFieldMismatch,
                "graph replay resource has invalid kind grammar",
            )
        })?;
    resource.parse()
}

#[cfg(test)]
#[path = "replay_tests.rs"]
mod tests;
