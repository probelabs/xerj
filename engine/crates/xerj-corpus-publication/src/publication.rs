use crate::{
    canonical_json::{self, JsonValue},
    codec::Encoder,
    digest::{
        CatalogGenerationIncarnationId, CatalogIdDigest, CatalogProjectionDigest,
        CatalogSealDigest, CatalogWrapperDigest, CorpusIncarnationId, CorpusOwnerId,
        DataContentDigest, DataIdDigest, DataProjectionDigest, DataSealDigest, DesiredPlanDigest,
        EdgePhysicalIdSetDigest, EdgeSealDigest, ExpectedPublicationDigest, GenerationId,
        GraphCoreDigest, GraphProjectionDigest, GraphToken, LogicalEdgeSetDigest,
        LogicalNodeSetDigest, ManifestDigest, MappingDigest, NodePhysicalIdSetDigest,
        NodeSealDigest, ProducerId, PublicationDigest, StorageIncarnation, TransactionId,
    },
    error::{error, ProtocolError, ProtocolErrorKind, Result},
    identity,
    projection::{CATALOG_INDEX, NODE_INDEX},
    scalar::{
        CorpusPrefix, DataSlug, Generation, LogicalIndexName, PhysicalDataName, RootIdentity,
        Sequence,
    },
};
use std::{fmt, str::FromStr};

pub struct CorpusPublicationJsonBytes(Box<[u8]>);
impl CorpusPublicationJsonBytes {
    pub fn canonical_json(&self) -> &[u8] {
        &self.0
    }
}
impl PartialEq for CorpusPublicationJsonBytes {
    fn eq(&self, o: &Self) -> bool {
        self.0 == o.0
    }
}
impl Eq for CorpusPublicationJsonBytes {}
impl fmt::Debug for CorpusPublicationJsonBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CorpusPublicationJsonBytes")
            .field("len", &self.0.len())
            .finish()
    }
}

struct Seal<D> {
    version: u32,
    sequence: u64,
    digest: D,
}
struct PubDataEntry {
    slug: DataSlug,
    logical: LogicalIndexName,
    physical: PhysicalDataName,
    storage: StorageIncarnation,
    mapping: MappingDigest,
    count: u64,
    ids: DataIdDigest,
    content: DataContentDigest,
    seal: Seal<DataSealDigest>,
}
struct PubData {
    generation: Generation,
    projection: DataProjectionDigest,
    indices: Vec<PubDataEntry>,
}
struct PubCatalog {
    storage_index: Box<str>,
    storage: StorageIncarnation,
    generation_id: GenerationId,
    incarnation: CatalogGenerationIncarnationId,
    mapping: MappingDigest,
    count: u64,
    ids: CatalogIdDigest,
    content: CatalogWrapperDigest,
    projection: CatalogProjectionDigest,
    seal: Seal<CatalogSealDigest>,
}
struct PubGraph {
    brain: Box<str>,
    owner: CorpusOwnerId,
    generation: Generation,
    producer: ProducerId,
    core: GraphCoreDigest,
    token: GraphToken,
    edges_index: Box<str>,
    edges_storage: StorageIncarnation,
    nodes_index: Box<str>,
    nodes_storage: StorageIncarnation,
    edge_mapping: MappingDigest,
    node_mapping: MappingDigest,
    edge_count: u64,
    logical_edges: LogicalEdgeSetDigest,
    edge_ids: EdgePhysicalIdSetDigest,
    node_count: u64,
    logical_nodes: LogicalNodeSetDigest,
    node_ids: NodePhysicalIdSetDigest,
    projection: GraphProjectionDigest,
    edge_seal: Seal<EdgeSealDigest>,
    node_seal: Seal<NodeSealDigest>,
}

pub struct CorpusPublicationV1 {
    canonical: CorpusPublicationJsonBytes,
    owner: CorpusOwnerId,
    prefix: CorpusPrefix,
    root: RootIdentity,
    incarnation: CorpusIncarnationId,
    sequence: Sequence,
    tx: TransactionId,
    manifest: ManifestDigest,
    plan: DesiredPlanDigest,
    data: PubData,
    catalog: PubCatalog,
    graph: PubGraph,
    digest: PublicationDigest,
}
impl fmt::Debug for CorpusPublicationV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CorpusPublicationV1")
            .field("owner", &self.owner)
            .field("sequence", &self.sequence)
            .field("digest", &self.digest)
            .finish_non_exhaustive()
    }
}
impl CorpusPublicationV1 {
    pub fn parse_closed_json(input: &[u8]) -> Result<Self, ProtocolError> {
        let value = canonical_json::parse(input, "publication")?;
        parse_publication_value(&value)
    }
    pub fn canonical_json(&self) -> &CorpusPublicationJsonBytes {
        &self.canonical
    }
    pub fn owner(&self) -> &CorpusOwnerId {
        &self.owner
    }
    pub fn root_identity(&self) -> &RootIdentity {
        &self.root
    }
    pub fn prefix(&self) -> &CorpusPrefix {
        &self.prefix
    }
    pub fn corpus_incarnation(&self) -> &CorpusIncarnationId {
        &self.incarnation
    }
    pub fn sequence(&self) -> Sequence {
        self.sequence
    }
    pub fn digest(&self) -> &PublicationDigest {
        &self.digest
    }
}

const PUB_FIELDS: &[&str] = &[
    "format_version",
    "owner",
    "prefix",
    "root_identity",
    "incarnation",
    "sequence",
    "tx_id",
    "manifest_digest",
    "plan_digest",
    "publication_digest",
    "data",
    "catalog",
    "graph",
];
fn parse_publication_value(value: &JsonValue) -> Result<CorpusPublicationV1> {
    let f = canonical_json::closed(value, "publication", PUB_FIELDS)?;
    if canonical_json::u64(f[0], "publication.format_version")? != 1 {
        return Err(error(
            ProtocolErrorKind::InvalidVersion,
            "publication version must equal 1",
        ));
    }
    let owner = f[1].as_digest()?;
    let prefix = CorpusPrefix::from_str(canonical_json::string(f[2], "publication.prefix")?)?;
    let root = RootIdentity::from_str(canonical_json::string(f[3], "publication.root_identity")?)?;
    let incarnation = f[4].as_digest()?;
    let sequence = Sequence::new(canonical_json::u64(f[5], "publication.sequence")?);
    let tx = f[6].as_digest()?;
    let manifest = f[7].as_digest()?;
    let plan = f[8].as_digest()?;
    let attached: PublicationDigest = f[9].as_digest()?;
    if identity::owner(&root, &prefix) != owner {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "publication owner does not match root/prefix",
        ));
    }
    let data = parse_data(f[10], &owner, &prefix, &incarnation, &tx, &manifest)?;
    let generation = data.generation;
    let catalog = parse_catalog(f[11], &owner, &incarnation, &tx, generation)?;
    let graph = parse_graph(f[12], &owner, &prefix, &incarnation, &tx, generation)?;
    validate_seals(&owner, &incarnation, &tx, &data, &catalog, &graph)?;
    let body = encode_publication_body(
        &owner,
        &prefix,
        &root,
        &incarnation,
        sequence,
        &tx,
        &manifest,
        &plan,
        &data,
        &catalog,
        &graph,
    );
    let digest = PublicationDigest::from_preimage(&body);
    if digest != attached {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "publication digest does not recompute",
        ));
    }
    let ordered = order_publication(value)?;
    let canonical =
        CorpusPublicationJsonBytes(canonical_json::serialize_in_order(&ordered).into_boxed_slice());
    Ok(CorpusPublicationV1 {
        canonical,
        owner,
        prefix,
        root,
        incarnation,
        sequence,
        tx,
        manifest,
        plan,
        data,
        catalog,
        graph,
        digest,
    })
}

trait DigestValue {
    fn as_digest<T: FromStr<Err = ProtocolError>>(&self) -> Result<T>;
}
impl DigestValue for JsonValue {
    fn as_digest<T: FromStr<Err = ProtocolError>>(&self) -> Result<T> {
        canonical_json::string(self, "digest")?.parse()
    }
}
fn parse_seal<D: FromStr<Err = ProtocolError>>(value: &JsonValue, field: &str) -> Result<Seal<D>> {
    let f = canonical_json::closed(
        value,
        field,
        &["seal_version", "final_write_sequence", "seal_digest"],
    )?;
    let version = u32::try_from(canonical_json::u64(f[0], field)?).map_err(|_| {
        error(
            ProtocolErrorKind::InvalidVersion,
            "seal version exceeds u32",
        )
    })?;
    if version != 1 {
        return Err(error(
            ProtocolErrorKind::InvalidVersion,
            "seal version must equal 1",
        ));
    }
    Ok(Seal {
        version,
        sequence: canonical_json::u64(f[1], field)?,
        digest: f[2].as_digest()?,
    })
}
fn parse_data(
    value: &JsonValue,
    owner: &CorpusOwnerId,
    prefix: &CorpusPrefix,
    inc: &CorpusIncarnationId,
    tx: &TransactionId,
    manifest: &ManifestDigest,
) -> Result<PubData> {
    let f = canonical_json::closed(
        value,
        "publication.data",
        &["generation", "projection_digest", "indices"],
    )?;
    let generation = Generation::new(canonical_json::u64(f[0], "publication.data.generation")?);
    let projection = f[1].as_digest()?;
    let mut indices = Vec::new();
    let mut last = None;
    for value in canonical_json::array(f[2], "publication.data.indices")? {
        let x = canonical_json::closed(
            value,
            "publication.data.indices[]",
            &[
                "slug",
                "logical_index",
                "physical_index",
                "physical_index_incarnation",
                "mapping_digest",
                "document_count",
                "id_digest",
                "content_digest",
                "seal",
            ],
        )?;
        let slug = DataSlug::from_str(canonical_json::string(x[0], "data.slug")?)?;
        let logical = LogicalIndexName::from_str(canonical_json::string(x[1], "data.logical")?)?;
        let physical = PhysicalDataName::from_str(canonical_json::string(x[2], "data.physical")?)?;
        if last
            .as_deref()
            .is_some_and(|v: &str| v >= slug.as_protocol_str())
        {
            return Err(error(
                ProtocolErrorKind::NonCanonicalEncoding,
                "publication data indices are not strictly sorted",
            ));
        }
        last = Some(slug.as_protocol_str().to_owned());
        if logical.as_protocol_str()
            != format!("{}-{}", prefix.as_protocol_str(), slug.as_protocol_str())
            || identity::physical_data_name(owner, inc, tx, manifest, generation, &slug)?
                != physical
        {
            return Err(error(
                ProtocolErrorKind::CrossFieldMismatch,
                "publication data route/name mismatch",
            ));
        }
        indices.push(PubDataEntry {
            slug,
            logical,
            physical,
            storage: x[3].as_digest()?,
            mapping: x[4].as_digest()?,
            count: canonical_json::u64(x[5], "data.count")?,
            ids: x[6].as_digest()?,
            content: x[7].as_digest()?,
            seal: parse_seal(x[8], "data.seal")?,
        });
    }
    let mut p = Encoder::domain(b"xerj-data-projection-v1\0");
    p.u64(generation.get());
    p.array_len(indices.len());
    for x in &indices {
        p.string(x.slug.as_protocol_str());
        p.string(x.logical.as_protocol_str());
        p.string(x.physical.as_protocol_str());
        p.string(x.mapping.as_rendered_str());
        p.u64(x.count);
        p.string(x.ids.as_rendered_str());
        p.string(x.content.as_rendered_str());
    }
    if DataProjectionDigest::from_preimage(&p.finish()) != projection {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "publication data projection mismatch",
        ));
    }
    Ok(PubData {
        generation,
        projection,
        indices,
    })
}
fn parse_catalog(
    value: &JsonValue,
    owner: &CorpusOwnerId,
    inc: &CorpusIncarnationId,
    tx: &TransactionId,
    generation: Generation,
) -> Result<PubCatalog> {
    let f = canonical_json::closed(
        value,
        "publication.catalog",
        &[
            "storage_index",
            "storage_incarnation",
            "generation_id",
            "incarnation",
            "mapping_digest",
            "document_count",
            "id_digest",
            "content_digest",
            "projection_digest",
            "seal",
        ],
    )?;
    let result = PubCatalog {
        storage_index: canonical_json::string(f[0], "catalog.storage_index")?.into(),
        storage: f[1].as_digest()?,
        generation_id: f[2].as_digest()?,
        incarnation: f[3].as_digest()?,
        mapping: f[4].as_digest()?,
        count: canonical_json::u64(f[5], "catalog.count")?,
        ids: f[6].as_digest()?,
        content: f[7].as_digest()?,
        projection: f[8].as_digest()?,
        seal: parse_seal(f[9], "catalog.seal")?,
    };
    if &*result.storage_index != CATALOG_INDEX
        || identity::generation_id(owner, inc, generation, tx) != result.generation_id
    {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "catalog generation identity mismatch",
        ));
    }
    let mut p = Encoder::domain(b"xerj-catalog-projection-v1\0");
    p.string(owner.as_rendered_str());
    p.string(inc.as_rendered_str());
    p.u64(generation.get());
    p.string(result.generation_id.as_rendered_str());
    p.u64(result.count);
    p.string(result.content.as_rendered_str());
    if CatalogProjectionDigest::from_preimage(&p.finish()) != result.projection
        || identity::catalog_incarnation(owner, inc, generation, tx, &result.projection)
            != result.incarnation
    {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "catalog projection/incarnation mismatch",
        ));
    }
    Ok(result)
}
fn parse_graph(
    value: &JsonValue,
    owner: &CorpusOwnerId,
    prefix: &CorpusPrefix,
    inc: &CorpusIncarnationId,
    tx: &TransactionId,
    generation: Generation,
) -> Result<PubGraph> {
    let f = canonical_json::closed(
        value,
        "publication.graph",
        &[
            "brain",
            "owner",
            "generation",
            "producer",
            "core_digest",
            "active_token",
            "edges_index",
            "edges_index_incarnation",
            "nodes_index",
            "nodes_index_incarnation",
            "edge_mapping_digest",
            "node_mapping_digest",
            "edge_count",
            "logical_edge_digest",
            "edge_physical_id_digest",
            "node_count",
            "logical_node_digest",
            "node_physical_id_digest",
            "projection_digest",
            "edge_seal",
            "node_seal",
        ],
    )?;
    let result = PubGraph {
        brain: canonical_json::string(f[0], "graph.brain")?.into(),
        owner: f[1].as_digest()?,
        generation: Generation::new(canonical_json::u64(f[2], "graph.generation")?),
        producer: f[3].as_digest()?,
        core: f[4].as_digest()?,
        token: f[5].as_digest()?,
        edges_index: canonical_json::string(f[6], "graph.edges_index")?.into(),
        edges_storage: f[7].as_digest()?,
        nodes_index: canonical_json::string(f[8], "graph.nodes_index")?.into(),
        nodes_storage: f[9].as_digest()?,
        edge_mapping: f[10].as_digest()?,
        node_mapping: f[11].as_digest()?,
        edge_count: canonical_json::u64(f[12], "graph.edge_count")?,
        logical_edges: f[13].as_digest()?,
        edge_ids: f[14].as_digest()?,
        node_count: canonical_json::u64(f[15], "graph.node_count")?,
        logical_nodes: f[16].as_digest()?,
        node_ids: f[17].as_digest()?,
        projection: f[18].as_digest()?,
        edge_seal: parse_seal(f[19], "graph.edge_seal")?,
        node_seal: parse_seal(f[20], "graph.node_seal")?,
    };
    if result.owner != *owner
        || &*result.brain != prefix.as_protocol_str()
        || result.generation != generation
        || *result.edges_index != format!(".xerj-memory-{}-edges", result.brain)
        || &*result.nodes_index != NODE_INDEX
    {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "graph publication join mismatch",
        ));
    }
    let mut core = Encoder::domain(b"xerj-graph-projection-core-v1\0");
    crate::prepared::encode_graph_core_body(
        &mut core,
        &result.brain,
        owner,
        &result.producer,
        result.edge_count,
        &result.logical_edges,
        result.node_count,
        &result.logical_nodes,
    );
    if GraphCoreDigest::from_preimage(&core.finish()) != result.core
        || identity::graph_token(owner, inc, result.generation, tx, &result.core) != result.token
    {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "graph core/token mismatch",
        ));
    }
    let mut p = Encoder::domain(b"xerj-graph-projection-v1\0");
    crate::prepared::encode_graph_core_body(
        &mut p,
        &result.brain,
        owner,
        &result.producer,
        result.edge_count,
        &result.logical_edges,
        result.node_count,
        &result.logical_nodes,
    );
    p.string(result.core.as_rendered_str());
    p.u64(result.generation.get());
    p.string(result.token.as_rendered_str());
    p.string(result.edge_ids.as_rendered_str());
    p.string(result.node_ids.as_rendered_str());
    if GraphProjectionDigest::from_preimage(&p.finish()) != result.projection {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "graph projection mismatch",
        ));
    }
    Ok(result)
}

#[allow(clippy::too_many_arguments)] // Mirrors the closed normative SealBody field order.
fn seal_preimage(
    domain: &'static [u8],
    owner: &CorpusOwnerId,
    inc: &CorpusIncarnationId,
    tx: &TransactionId,
    generation: u64,
    kind: &str,
    storage_name: &str,
    storage: &StorageIncarnation,
    final_sequence: u64,
    mapping: &MappingDigest,
    count: u64,
    ids: &str,
    content: &str,
    projection: &str,
) -> Vec<u8> {
    let mut p = Encoder::domain(domain);
    p.string(owner.as_rendered_str());
    p.string(inc.as_rendered_str());
    p.string(tx.as_rendered_str());
    p.u64(generation);
    p.string(kind);
    p.string(storage_name);
    p.string(storage.as_rendered_str());
    p.u64(final_sequence);
    p.string(mapping.as_rendered_str());
    p.u64(count);
    p.string(ids);
    p.string(content);
    p.string(projection);
    p.finish()
}
fn validate_seals(
    owner: &CorpusOwnerId,
    inc: &CorpusIncarnationId,
    tx: &TransactionId,
    data: &PubData,
    catalog: &PubCatalog,
    graph: &PubGraph,
) -> Result<()> {
    for x in &data.indices {
        let p = seal_preimage(
            b"xerj-data-seal-v1\0",
            owner,
            inc,
            tx,
            data.generation.get(),
            "data",
            x.physical.as_protocol_str(),
            &x.storage,
            x.seal.sequence,
            &x.mapping,
            x.count,
            x.ids.as_rendered_str(),
            x.content.as_rendered_str(),
            data.projection.as_rendered_str(),
        );
        if DataSealDigest::from_preimage(&p) != x.seal.digest {
            return Err(error(
                ProtocolErrorKind::CrossFieldMismatch,
                "data seal mismatch",
            ));
        }
    }
    let p = seal_preimage(
        b"xerj-catalog-seal-v1\0",
        owner,
        inc,
        tx,
        data.generation.get(),
        "catalog",
        &catalog.storage_index,
        &catalog.storage,
        catalog.seal.sequence,
        &catalog.mapping,
        catalog.count,
        catalog.ids.as_rendered_str(),
        catalog.content.as_rendered_str(),
        catalog.projection.as_rendered_str(),
    );
    if CatalogSealDigest::from_preimage(&p) != catalog.seal.digest {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "catalog seal mismatch",
        ));
    }
    let p = seal_preimage(
        b"xerj-graph-edge-seal-v1\0",
        owner,
        inc,
        tx,
        graph.generation.get(),
        "graph-edge",
        &graph.edges_index,
        &graph.edges_storage,
        graph.edge_seal.sequence,
        &graph.edge_mapping,
        graph.edge_count,
        graph.edge_ids.as_rendered_str(),
        graph.logical_edges.as_rendered_str(),
        graph.projection.as_rendered_str(),
    );
    if EdgeSealDigest::from_preimage(&p) != graph.edge_seal.digest {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "graph edge seal mismatch",
        ));
    }
    let p = seal_preimage(
        b"xerj-graph-node-seal-v1\0",
        owner,
        inc,
        tx,
        graph.generation.get(),
        "graph-node",
        &graph.nodes_index,
        &graph.nodes_storage,
        graph.node_seal.sequence,
        &graph.node_mapping,
        graph.node_count,
        graph.node_ids.as_rendered_str(),
        graph.logical_nodes.as_rendered_str(),
        graph.projection.as_rendered_str(),
    );
    if NodeSealDigest::from_preimage(&p) != graph.node_seal.digest {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "graph node seal mismatch",
        ));
    }
    Ok(())
}
fn encode_seal<D: fmt::Display>(out: &mut Encoder, seal: &Seal<D>) {
    out.u32(seal.version);
    out.u64(seal.sequence);
    out.string(&seal.digest.to_string());
}
#[allow(clippy::too_many_arguments)] // Mirrors the closed normative CorpusPublicationBody.
fn encode_publication_body(
    owner: &CorpusOwnerId,
    prefix: &CorpusPrefix,
    root: &RootIdentity,
    inc: &CorpusIncarnationId,
    sequence: Sequence,
    tx: &TransactionId,
    manifest: &ManifestDigest,
    plan: &DesiredPlanDigest,
    data: &PubData,
    catalog: &PubCatalog,
    graph: &PubGraph,
) -> Vec<u8> {
    let mut out = Encoder::domain(b"xerj-corpus-publication-v1\0");
    out.u32(1);
    out.string(owner.as_rendered_str());
    out.string(prefix.as_protocol_str());
    out.string(root.as_protocol_str());
    out.string(inc.as_rendered_str());
    out.u64(sequence.get());
    out.string(tx.as_rendered_str());
    out.string(manifest.as_rendered_str());
    out.string(plan.as_rendered_str());
    out.u64(data.generation.get());
    out.string(data.projection.as_rendered_str());
    out.array_len(data.indices.len());
    for x in &data.indices {
        out.string(x.slug.as_protocol_str());
        out.string(x.logical.as_protocol_str());
        out.string(x.physical.as_protocol_str());
        out.string(x.storage.as_rendered_str());
        out.string(x.mapping.as_rendered_str());
        out.u64(x.count);
        out.string(x.ids.as_rendered_str());
        out.string(x.content.as_rendered_str());
        encode_seal(&mut out, &x.seal);
    }
    out.string(&catalog.storage_index);
    out.string(catalog.storage.as_rendered_str());
    out.string(catalog.generation_id.as_rendered_str());
    out.string(catalog.incarnation.as_rendered_str());
    out.string(catalog.mapping.as_rendered_str());
    out.u64(catalog.count);
    out.string(catalog.ids.as_rendered_str());
    out.string(catalog.content.as_rendered_str());
    out.string(catalog.projection.as_rendered_str());
    encode_seal(&mut out, &catalog.seal);
    out.string(&graph.brain);
    out.string(graph.owner.as_rendered_str());
    out.u64(graph.generation.get());
    out.string(graph.producer.as_rendered_str());
    out.string(graph.core.as_rendered_str());
    out.string(graph.token.as_rendered_str());
    out.string(&graph.edges_index);
    out.string(graph.edges_storage.as_rendered_str());
    out.string(&graph.nodes_index);
    out.string(graph.nodes_storage.as_rendered_str());
    out.string(graph.edge_mapping.as_rendered_str());
    out.string(graph.node_mapping.as_rendered_str());
    out.u64(graph.edge_count);
    out.string(graph.logical_edges.as_rendered_str());
    out.string(graph.edge_ids.as_rendered_str());
    out.u64(graph.node_count);
    out.string(graph.logical_nodes.as_rendered_str());
    out.string(graph.node_ids.as_rendered_str());
    out.string(graph.projection.as_rendered_str());
    encode_seal(&mut out, &graph.edge_seal);
    encode_seal(&mut out, &graph.node_seal);
    out.finish()
}

fn order_publication(value: &JsonValue) -> Result<JsonValue> {
    canonical_json::ordered_object(value, "publication", PUB_FIELDS, |name, v| match name {
        "data" => canonical_json::ordered_object(
            v,
            "data",
            &["generation", "projection_digest", "indices"],
            |n, x| {
                if n == "indices" {
                    Ok(JsonValue::Array(
                        canonical_json::array(x, "indices")?
                            .iter()
                            .map(order_data_entry)
                            .collect::<Result<_>>()?,
                    ))
                } else {
                    Ok(x.clone())
                }
            },
        ),
        "catalog" => order_simple(
            v,
            "catalog",
            &[
                "storage_index",
                "storage_incarnation",
                "generation_id",
                "incarnation",
                "mapping_digest",
                "document_count",
                "id_digest",
                "content_digest",
                "projection_digest",
                "seal",
            ],
            &["seal"],
        ),
        "graph" => order_simple(
            v,
            "graph",
            &[
                "brain",
                "owner",
                "generation",
                "producer",
                "core_digest",
                "active_token",
                "edges_index",
                "edges_index_incarnation",
                "nodes_index",
                "nodes_index_incarnation",
                "edge_mapping_digest",
                "node_mapping_digest",
                "edge_count",
                "logical_edge_digest",
                "edge_physical_id_digest",
                "node_count",
                "logical_node_digest",
                "node_physical_id_digest",
                "projection_digest",
                "edge_seal",
                "node_seal",
            ],
            &["edge_seal", "node_seal"],
        ),
        _ => Ok(v.clone()),
    })
}
fn order_data_entry(v: &JsonValue) -> Result<JsonValue> {
    order_simple(
        v,
        "data entry",
        &[
            "slug",
            "logical_index",
            "physical_index",
            "physical_index_incarnation",
            "mapping_digest",
            "document_count",
            "id_digest",
            "content_digest",
            "seal",
        ],
        &["seal"],
    )
}
fn order_simple(
    v: &JsonValue,
    field: &str,
    names: &[&str],
    seal_names: &[&str],
) -> Result<JsonValue> {
    canonical_json::ordered_object(v, field, names, |name, x| {
        if seal_names.contains(&name) {
            canonical_json::ordered_object(
                x,
                "seal",
                &["seal_version", "final_write_sequence", "seal_digest"],
                |_, y| Ok(y.clone()),
            )
        } else {
            Ok(x.clone())
        }
    })
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum ExpectedPublicationKind {
    Absent,
    Present,
}
pub struct ExpectedPublicationJsonBytes(Box<[u8]>);
impl ExpectedPublicationJsonBytes {
    pub fn canonical_json(&self) -> &[u8] {
        &self.0
    }
}
impl PartialEq for ExpectedPublicationJsonBytes {
    fn eq(&self, o: &Self) -> bool {
        self.0 == o.0
    }
}
impl Eq for ExpectedPublicationJsonBytes {}
impl fmt::Debug for ExpectedPublicationJsonBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExpectedPublicationJsonBytes")
            .field("len", &self.0.len())
            .finish()
    }
}
enum ExpectedBody {
    Absent(CorpusOwnerId),
    Present(Box<CorpusPublicationV1>),
}
pub struct ExpectedPublicationV1 {
    body: ExpectedBody,
    digest: ExpectedPublicationDigest,
    canonical: ExpectedPublicationJsonBytes,
}
impl fmt::Debug for ExpectedPublicationV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExpectedPublicationV1")
            .field("kind", &self.kind())
            .field("owner", &self.owner())
            .field("digest", &self.digest)
            .finish()
    }
}
impl ExpectedPublicationV1 {
    pub fn absent(owner: CorpusOwnerId) -> Self {
        let mut b = Encoder::default();
        b.u32(0);
        b.string(owner.as_rendered_str());
        b.u64(0);
        let digest = expected_digest(&b.finish());
        let json = format!(
            "{{\"kind\":\"absent\",\"owner\":{},\"sequence\":0}}",
            canonical_json::json_string(owner.as_rendered_str())
        );
        Self {
            body: ExpectedBody::Absent(owner),
            digest,
            canonical: ExpectedPublicationJsonBytes(json.into_bytes().into_boxed_slice()),
        }
    }
    pub fn present(publication: CorpusPublicationV1) -> Result<Self, ProtocolError> {
        let body = publication_body(&publication);
        let mut b = Encoder::default();
        b.u32(1);
        b.raw(&body);
        b.string(publication.digest.as_rendered_str());
        let digest = expected_digest(&b.finish());
        let mut json = b"{\"kind\":\"present\",\"publication\":".to_vec();
        json.extend_from_slice(publication.canonical.canonical_json());
        json.push(b'}');
        Ok(Self {
            body: ExpectedBody::Present(Box::new(publication)),
            digest,
            canonical: ExpectedPublicationJsonBytes(json.into_boxed_slice()),
        })
    }
    pub fn parse_closed_json(input: &[u8]) -> Result<Self, ProtocolError> {
        let v = canonical_json::parse(input, "expected_publication")?;
        let kind = canonical_json::string(
            canonical_json::member(&v, "expected_publication", "kind")?,
            "expected_publication.kind",
        )?;
        match kind {
            "absent" => {
                let f = canonical_json::closed(
                    &v,
                    "expected_publication",
                    &["kind", "owner", "sequence"],
                )?;
                if canonical_json::u64(f[2], "expected_publication.sequence")? != 0 {
                    return Err(error(
                        ProtocolErrorKind::CrossFieldMismatch,
                        "absent expectation sequence must be zero",
                    ));
                }
                Ok(Self::absent(f[1].as_digest()?))
            }
            "present" => {
                let f =
                    canonical_json::closed(&v, "expected_publication", &["kind", "publication"])?;
                Self::present(parse_publication_value(f[1])?)
            }
            _ => Err(error(
                ProtocolErrorKind::InvalidScalar,
                "invalid expected publication kind",
            )),
        }
    }
    pub fn kind(&self) -> ExpectedPublicationKind {
        match self.body {
            ExpectedBody::Absent(_) => ExpectedPublicationKind::Absent,
            ExpectedBody::Present(_) => ExpectedPublicationKind::Present,
        }
    }
    pub fn owner(&self) -> &CorpusOwnerId {
        match &self.body {
            ExpectedBody::Absent(v) => v,
            ExpectedBody::Present(v) => &v.owner,
        }
    }
    pub fn digest(&self) -> &ExpectedPublicationDigest {
        &self.digest
    }
    pub fn canonical_json(&self) -> &ExpectedPublicationJsonBytes {
        &self.canonical
    }
    pub(crate) fn sequence(&self) -> Sequence {
        match &self.body {
            ExpectedBody::Absent(_) => Sequence::new(0),
            ExpectedBody::Present(v) => v.sequence,
        }
    }
    pub(crate) fn prefix(&self) -> Option<&CorpusPrefix> {
        match &self.body {
            ExpectedBody::Absent(_) => None,
            ExpectedBody::Present(v) => Some(&v.prefix),
        }
    }
    pub(crate) fn root(&self) -> Option<&RootIdentity> {
        match &self.body {
            ExpectedBody::Absent(_) => None,
            ExpectedBody::Present(v) => Some(&v.root),
        }
    }
    pub(crate) fn incarnation(&self) -> Option<&CorpusIncarnationId> {
        match &self.body {
            ExpectedBody::Absent(_) => None,
            ExpectedBody::Present(v) => Some(&v.incarnation),
        }
    }
    pub(crate) fn binary_body(&self) -> Vec<u8> {
        match &self.body {
            ExpectedBody::Absent(owner) => {
                let mut b = Encoder::default();
                b.u32(0);
                b.string(owner.as_rendered_str());
                b.u64(0);
                b.finish()
            }
            ExpectedBody::Present(v) => {
                let mut b = Encoder::default();
                b.u32(1);
                b.raw(&publication_body(v));
                b.string(v.digest.as_rendered_str());
                b.finish()
            }
        }
    }
}
fn publication_body(v: &CorpusPublicationV1) -> Vec<u8> {
    encode_publication_body(
        &v.owner,
        &v.prefix,
        &v.root,
        &v.incarnation,
        v.sequence,
        &v.tx,
        &v.manifest,
        &v.plan,
        &v.data,
        &v.catalog,
        &v.graph,
    )
}
fn expected_digest(body: &[u8]) -> ExpectedPublicationDigest {
    let mut p = Encoder::domain(b"xerj-expected-publication-v1\0");
    p.raw(body);
    ExpectedPublicationDigest::from_preimage(&p.finish())
}
