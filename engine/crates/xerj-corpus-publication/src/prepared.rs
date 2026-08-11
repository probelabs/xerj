use crate::{
    codec::{Cursor, Encoder},
    digest::{
        CatalogIdDigest, CatalogWrapperDigest, CorpusIncarnationId, CorpusOwnerId,
        DataContentDigest, DataIdDigest, GraphCoreDigest, LogicalEdgeSetDigest,
        LogicalNodeSetDigest, PreparedInputDigest, ProducerId, ReplayArtifactDigest,
    },
    error::{error, ProtocolError, ProtocolErrorKind, Result},
    identity,
    logical_input::{
        CatalogInputV1, DataRouteInputV1, GraphInputV1, LogicalEdgeRowV1, LogicalNodeRowV1,
        PrepareCorpusInputV1,
    },
    manifest::ManifestV1,
    scalar::{CorpusPrefix, RootIdentity, Sequence},
};
use std::{fmt, str::FromStr};

pub struct PreparedInputBytes(Box<[u8]>);
impl PreparedInputBytes {
    pub fn canonical_preimage(&self) -> &[u8] {
        &self.0
    }
}
impl PartialEq for PreparedInputBytes {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl Eq for PreparedInputBytes {}
impl fmt::Debug for PreparedInputBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreparedInputBytes")
            .field("len", &self.0.len())
            .finish()
    }
}

pub struct PreparedInputV1 {
    pub(crate) bytes: PreparedInputBytes,
    pub(crate) digest: PreparedInputDigest,
    pub(crate) summary: PreparedInputSummaryV1,
}
impl fmt::Debug for PreparedInputV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreparedInputV1")
            .field("bytes", &self.bytes)
            .field("digest", &self.digest)
            .finish()
    }
}
impl PreparedInputV1 {
    pub fn parse_canonical_preimage(input: &[u8]) -> Result<Self, ProtocolError> {
        let summary = parse_prepared_encoding(input)?;
        Ok(Self {
            bytes: PreparedInputBytes(input.into()),
            digest: PreparedInputDigest::from_preimage(input),
            summary,
        })
    }
    pub fn canonical_preimage(&self) -> &PreparedInputBytes {
        &self.bytes
    }
    pub fn digest(&self) -> &PreparedInputDigest {
        &self.digest
    }
}

pub(crate) struct PreparedDataSummaryV1 {
    pub(crate) slug: crate::scalar::DataSlug,
    pub(crate) mapping: crate::digest::MappingDigest,
    pub(crate) count: u64,
    pub(crate) ids: DataIdDigest,
    pub(crate) content: DataContentDigest,
    pub(crate) payload: ReplayArtifactDigest,
}

pub(crate) struct PreparedCatalogSummaryV1 {
    pub(crate) count: u64,
    pub(crate) ids: CatalogIdDigest,
    pub(crate) content: CatalogWrapperDigest,
    pub(crate) payload: ReplayArtifactDigest,
}

pub(crate) struct PreparedGraphSummaryV1 {
    pub(crate) brain: crate::scalar::BrainName,
    pub(crate) owner: CorpusOwnerId,
    pub(crate) producer: ProducerId,
    pub(crate) edge_count: u64,
    pub(crate) logical_edges: LogicalEdgeSetDigest,
    pub(crate) node_count: u64,
    pub(crate) logical_nodes: LogicalNodeSetDigest,
    pub(crate) core: GraphCoreDigest,
}

pub(crate) struct PreparedInputSummaryV1 {
    pub(crate) owner: CorpusOwnerId,
    pub(crate) incarnation: CorpusIncarnationId,
    pub(crate) manifest: crate::digest::ManifestDigest,
    pub(crate) data: Vec<PreparedDataSummaryV1>,
    pub(crate) catalog: PreparedCatalogSummaryV1,
    pub(crate) graph: PreparedGraphSummaryV1,
}

pub(crate) struct PreparedData {
    pub(crate) input: DataRouteInputV1,
    pub(crate) id_digest: DataIdDigest,
    pub(crate) content_digest: DataContentDigest,
    pub(crate) payload_digest: ReplayArtifactDigest,
}
pub(crate) struct PreparedCatalog {
    pub(crate) input: CatalogInputV1,
    pub(crate) id_digest: CatalogIdDigest,
    pub(crate) wrapper_digest: CatalogWrapperDigest,
    pub(crate) payload_digest: ReplayArtifactDigest,
}
pub(crate) struct PreparedGraph {
    pub(crate) input: GraphInputV1,
    pub(crate) producer: ProducerId,
    pub(crate) logical_edge_digest: LogicalEdgeSetDigest,
    pub(crate) logical_node_digest: LogicalNodeSetDigest,
    pub(crate) core_digest: GraphCoreDigest,
}

pub struct PreparedCorpusV1 {
    pub(crate) root_identity: RootIdentity,
    pub(crate) prefix: CorpusPrefix,
    pub(crate) owner: CorpusOwnerId,
    pub(crate) incarnation: CorpusIncarnationId,
    pub(crate) manifest: ManifestV1,
    pub(crate) data: Vec<PreparedData>,
    pub(crate) catalog: PreparedCatalog,
    pub(crate) graph: PreparedGraph,
    pub(crate) prepared_input: PreparedInputV1,
}
impl fmt::Debug for PreparedCorpusV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreparedCorpusV1")
            .field("owner", &self.owner)
            .field("incarnation", &self.incarnation)
            .field("prepared_input", &self.prepared_input)
            .finish_non_exhaustive()
    }
}

impl PreparedCorpusV1 {
    pub fn prepare(input: PrepareCorpusInputV1) -> Result<Self, ProtocolError> {
        let owner = identity::owner(&input.root_identity, &input.prefix);
        let incarnation = identity::corpus_incarnation(&owner, input.corpus_seed.consume());
        let data = input
            .data
            .into_iter()
            .map(prepare_data)
            .collect::<Result<Vec<_>>>()?;
        let catalog = prepare_catalog(input.catalog)?;
        let graph = prepare_graph(input.graph, &owner)?;
        let bytes = encode_prepared(
            &owner,
            &incarnation,
            input.manifest.digest(),
            &data,
            &catalog,
            &graph,
        );
        // Fresh construction and persisted recovery retain the same parsed
        // summary. This deliberately reparses the generated preimage instead
        // of trusting the logical-input objects as a separate fast path.
        let prepared_input = PreparedInputV1::parse_canonical_preimage(&bytes)?;
        Ok(Self {
            root_identity: input.root_identity,
            prefix: input.prefix,
            owner,
            incarnation,
            manifest: input.manifest,
            data,
            catalog,
            graph,
            prepared_input,
        })
    }
    pub fn owner(&self) -> &CorpusOwnerId {
        &self.owner
    }
    pub fn corpus_incarnation(&self) -> &CorpusIncarnationId {
        &self.incarnation
    }
    pub fn prepared_input(&self) -> &PreparedInputV1 {
        &self.prepared_input
    }
}

pub struct SequenceTransitionV1 {
    expected: Sequence,
    desired: Sequence,
}
impl SequenceTransitionV1 {
    pub fn new(expected: Sequence, desired: Sequence) -> Result<Self, ProtocolError> {
        if expected.get().checked_add(1) != Some(desired.get()) {
            return Err(error(
                ProtocolErrorKind::CrossFieldMismatch,
                "desired sequence must be checked successor of expected sequence",
            ));
        }
        Ok(Self { expected, desired })
    }
    pub fn expected(&self) -> Sequence {
        self.expected
    }
    pub fn desired(&self) -> Sequence {
        self.desired
    }
}

fn digest_array<T>(
    domain: &'static [u8],
    values: &[T],
    mut write: impl FnMut(&mut Encoder, &T),
) -> Vec<u8> {
    let mut bytes = Encoder::domain(domain);
    bytes.array_len(values.len());
    for value in values {
        write(&mut bytes, value);
    }
    bytes.finish()
}

fn prepare_data(input: DataRouteInputV1) -> Result<PreparedData> {
    let id = digest_array(b"xerj-id-set-v1\0", &input.documents, |out, doc| {
        out.string(doc.id.as_protocol_str())
    });
    let content = digest_array(b"xerj-data-content-v1\0", &input.documents, |out, doc| {
        out.string(doc.id.as_protocol_str());
        out.bytes(&doc.source.canonical);
    });
    let mut payload = Vec::new();
    for doc in &input.documents {
        payload.extend_from_slice(
            format!(
                "{{\"id\":{}}}\n",
                crate::canonical_json::json_string(doc.id.as_protocol_str())
            )
            .as_bytes(),
        );
        payload.extend_from_slice(&doc.source.canonical);
        payload.push(b'\n');
    }
    Ok(PreparedData {
        id_digest: DataIdDigest::from_preimage(&id),
        content_digest: DataContentDigest::from_preimage(&content),
        payload_digest: artifact_digest("prepared-data-rows", &payload),
        input,
    })
}

fn prepare_catalog(input: CatalogInputV1) -> Result<PreparedCatalog> {
    let id = digest_array(b"xerj-catalog-id-set-v1\0", &input.wrappers, |out, row| {
        out.string(row.id.as_protocol_str())
    });
    let wrappers = digest_array(
        b"xerj-catalog-wrapper-set-v1\0",
        &input.wrappers,
        |out, row| {
            out.string(row.id.as_protocol_str());
            out.bytes(&row.source.canonical);
        },
    );
    let mut payload = Vec::new();
    for row in &input.wrappers {
        payload.extend_from_slice(
            format!(
                "{{\"id\":{}}}\n",
                crate::canonical_json::json_string(row.id.as_protocol_str())
            )
            .as_bytes(),
        );
        payload.extend_from_slice(&row.source.canonical);
        payload.push(b'\n');
    }
    Ok(PreparedCatalog {
        id_digest: CatalogIdDigest::from_preimage(&id),
        wrapper_digest: CatalogWrapperDigest::from_preimage(&wrappers),
        payload_digest: artifact_digest("prepared-catalog-rows", &payload),
        input,
    })
}

fn prepare_graph(input: GraphInputV1, owner: &CorpusOwnerId) -> Result<PreparedGraph> {
    let mut producer_bytes = Encoder::domain(b"xerj-autoindex-producer-v1\0");
    producer_bytes.string(owner.as_rendered_str());
    producer_bytes.string(input.brain.as_protocol_str());
    producer_bytes.string(input.extractor_identity.as_protocol_str());
    producer_bytes.string(input.extractor_config.digest.as_rendered_str());
    let producer = ProducerId::from_preimage(&producer_bytes.finish());
    let edge_bytes = digest_array(
        b"xerj-graph-logical-edges-v1\0",
        &input.edges,
        |out: &mut Encoder, row: &LogicalEdgeRowV1| {
            out.string(row.logical_id.as_lower_hex());
            out.bytes(&row.canonical);
        },
    );
    let node_bytes = digest_array(
        b"xerj-graph-logical-nodes-v1\0",
        &input.nodes,
        |out: &mut Encoder, row: &LogicalNodeRowV1| {
            out.string(&row.source_index);
            out.string(&row.logical_node_id);
            out.bytes(&row.canonical);
        },
    );
    let logical_edge_digest = LogicalEdgeSetDigest::from_preimage(&edge_bytes);
    let logical_node_digest = LogicalNodeSetDigest::from_preimage(&node_bytes);
    let mut core = Encoder::domain(b"xerj-graph-projection-core-v1\0");
    encode_graph_core_body(
        &mut core,
        input.brain.as_protocol_str(),
        owner,
        &producer,
        input.edges.len() as u64,
        &logical_edge_digest,
        input.nodes.len() as u64,
        &logical_node_digest,
    );
    let core_digest = GraphCoreDigest::from_preimage(&core.finish());
    Ok(PreparedGraph {
        input,
        producer,
        logical_edge_digest,
        logical_node_digest,
        core_digest,
    })
}

pub(crate) fn artifact_digest(kind: &str, payload: &[u8]) -> ReplayArtifactDigest {
    let mut bytes = Encoder::domain(b"xerj-replay-artifact-v1\0");
    bytes.string(kind);
    bytes.u64(payload.len() as u64);
    bytes.raw(payload);
    ReplayArtifactDigest::from_preimage(&bytes.finish())
}

#[allow(clippy::too_many_arguments)] // The normative preimage has seven ordered fields plus its sink.
pub(crate) fn encode_graph_core_body(
    out: &mut Encoder,
    brain: &str,
    owner: &CorpusOwnerId,
    producer: &ProducerId,
    edge_count: u64,
    edge_digest: &LogicalEdgeSetDigest,
    node_count: u64,
    node_digest: &LogicalNodeSetDigest,
) {
    out.string(brain);
    out.string(owner.as_rendered_str());
    out.string(producer.as_rendered_str());
    out.u64(edge_count);
    out.string(edge_digest.as_rendered_str());
    out.u64(node_count);
    out.string(node_digest.as_rendered_str());
}

fn encode_prepared(
    owner: &CorpusOwnerId,
    incarnation: &CorpusIncarnationId,
    manifest: &crate::digest::ManifestDigest,
    data: &[PreparedData],
    catalog: &PreparedCatalog,
    graph: &PreparedGraph,
) -> Vec<u8> {
    let mut out = Encoder::domain(b"xerj-prepared-input-v1\0");
    out.u32(1);
    out.string(owner.as_rendered_str());
    out.string(incarnation.as_rendered_str());
    out.string(manifest.as_rendered_str());
    out.array_len(data.len());
    for row in data {
        out.string(row.input.slug.as_protocol_str());
        out.string(row.input.mapping.digest.as_rendered_str());
        out.u64(row.input.documents.len() as u64);
        out.string(row.id_digest.as_rendered_str());
        out.string(row.content_digest.as_rendered_str());
        out.string(row.payload_digest.as_rendered_str());
    }
    out.u64(catalog.input.wrappers.len() as u64);
    out.string(catalog.id_digest.as_rendered_str());
    out.string(catalog.wrapper_digest.as_rendered_str());
    out.string(catalog.payload_digest.as_rendered_str());
    encode_graph_core_body(
        &mut out,
        graph.input.brain.as_protocol_str(),
        owner,
        &graph.producer,
        graph.input.edges.len() as u64,
        &graph.logical_edge_digest,
        graph.input.nodes.len() as u64,
        &graph.logical_node_digest,
    );
    out.finish()
}

pub(crate) fn parse_prepared_encoding(input: &[u8]) -> Result<PreparedInputSummaryV1> {
    let mut c = Cursor::new(input);
    c.domain(b"xerj-prepared-input-v1\0")?;
    if c.u32("format_version")? != 1 {
        return Err(error(
            ProtocolErrorKind::InvalidVersion,
            "prepared input version must equal 1",
        ));
    }
    let owner: CorpusOwnerId = c.string("owner")?.parse()?;
    let incarnation: CorpusIncarnationId = c.string("incarnation")?.parse()?;
    let manifest: crate::digest::ManifestDigest = c.string("manifest")?.parse()?;
    let count = c.len("prepared_data")?;
    let mut last: Option<String> = None;
    let mut data = Vec::with_capacity(count);
    for _ in 0..count {
        let slug = crate::scalar::DataSlug::from_str(c.string("slug")?)?;
        if last.as_deref().is_some_and(|v| v >= slug.as_protocol_str()) {
            return Err(error(
                ProtocolErrorKind::NonCanonicalEncoding,
                "prepared data routes are not strictly sorted",
            ));
        }
        last = Some(slug.as_protocol_str().to_owned());
        data.push(PreparedDataSummaryV1 {
            slug,
            mapping: c.string("mapping")?.parse()?,
            count: c.u64("document_count")?,
            ids: c.string("id_digest")?.parse()?,
            content: c.string("content_digest")?.parse()?,
            payload: c.string("payload_digest")?.parse()?,
        });
    }
    let catalog = PreparedCatalogSummaryV1 {
        count: c.u64("catalog_count")?,
        ids: c.string("catalog_id")?.parse()?,
        content: c.string("catalog_content")?.parse()?,
        payload: c.string("catalog_payload")?.parse()?,
    };
    let brain = crate::scalar::BrainName::from_str(c.string("brain")?)?;
    let graph_owner: CorpusOwnerId = c.string("graph.owner")?.parse()?;
    if graph_owner != owner {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "prepared graph owner does not match prepared owner",
        ));
    }
    let producer: ProducerId = c.string("producer")?.parse()?;
    let edge_count = c.u64("edge_count")?;
    let logical_edges: LogicalEdgeSetDigest = c.string("edges")?.parse()?;
    let node_count = c.u64("node_count")?;
    let logical_nodes: LogicalNodeSetDigest = c.string("nodes")?.parse()?;
    c.finish()?;
    let mut core = Encoder::domain(b"xerj-graph-projection-core-v1\0");
    encode_graph_core_body(
        &mut core,
        brain.as_protocol_str(),
        &owner,
        &producer,
        edge_count,
        &logical_edges,
        node_count,
        &logical_nodes,
    );
    Ok(PreparedInputSummaryV1 {
        owner,
        incarnation,
        manifest,
        data,
        catalog,
        graph: PreparedGraphSummaryV1 {
            brain,
            owner: graph_owner,
            producer,
            edge_count,
            logical_edges,
            node_count,
            logical_nodes,
            core: GraphCoreDigest::from_preimage(&core.finish()),
        },
    })
}
