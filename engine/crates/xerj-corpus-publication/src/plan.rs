use crate::{
    canonical_json,
    codec::{Cursor, Encoder},
    digest::{
        CatalogGenerationIncarnationId, CatalogIdDigest, CatalogProjectionDigest,
        CatalogWrapperDigest, CorpusIncarnationId, CorpusOwnerId, DataContentDigest, DataIdDigest,
        DataProjectionDigest, DesiredPlanDigest, EdgePhysicalIdSetDigest, GenerationId,
        GraphCoreDigest, GraphProjectionDigest, GraphToken, LogicalEdgeSetDigest,
        LogicalNodeSetDigest, ManifestDigest, MappingDigest, NodePhysicalIdSetDigest,
        PreparedInputDigest, ProducerId, ReplayArtifactDigest, ReplaySetDigest, TransactionId,
    },
    error::{error, ProtocolError, ProtocolErrorKind, Result},
    identity,
    prepared::{PreparedCorpusV1, PreparedInputV1, SequenceTransitionV1},
    projection::{self, DerivedPlan, CATALOG_INDEX, NODE_INDEX},
    replay::{ProjectionKind, ReplayArtifactKind, ReplayArtifactV1},
    scalar::{
        BrainName, CorpusPrefix, DataSlug, Generation, LogicalIndexName, PhysicalDataName,
        ResourceKey, RootIdentity, Sequence,
    },
};
use std::{fmt, str::FromStr};

pub struct DesiredPlanBytes(Box<[u8]>);
impl DesiredPlanBytes {
    pub fn canonical_preimage(&self) -> &[u8] {
        &self.0
    }
}

pub struct MappingJsonBytes(Box<[u8]>);
impl MappingJsonBytes {
    pub fn canonical_json(&self) -> &[u8] {
        &self.0
    }
}
impl PartialEq for MappingJsonBytes {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl Eq for MappingJsonBytes {}
impl fmt::Debug for MappingJsonBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MappingJsonBytes")
            .field("len", &self.0.len())
            .finish()
    }
}

pub struct MappingReservationV1 {
    projection_kind: ProjectionKind,
    resource_key: ResourceKey,
    mapping_digest: MappingDigest,
    canonical_mapping_json: MappingJsonBytes,
}
impl MappingReservationV1 {
    pub fn projection_kind(&self) -> ProjectionKind {
        self.projection_kind
    }
    pub fn resource_key(&self) -> &ResourceKey {
        &self.resource_key
    }
    pub fn mapping_digest(&self) -> &MappingDigest {
        &self.mapping_digest
    }
    pub fn canonical_mapping_json(&self) -> &MappingJsonBytes {
        &self.canonical_mapping_json
    }
    pub(crate) fn from_canonical_mapping(
        projection_kind: ProjectionKind,
        resource_key: ResourceKey,
        mapping_digest: MappingDigest,
        canonical_mapping_json: Box<[u8]>,
    ) -> Result<Self> {
        validate_canonical_mapping(&canonical_mapping_json, &mapping_digest)?;
        Ok(Self {
            projection_kind,
            resource_key,
            mapping_digest,
            canonical_mapping_json: MappingJsonBytes(canonical_mapping_json),
        })
    }
}
impl fmt::Debug for MappingReservationV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MappingReservationV1")
            .field("projection_kind", &self.projection_kind)
            .field("resource_key", &self.resource_key)
            .field("mapping_digest", &self.mapping_digest)
            .field("canonical_mapping_json", &self.canonical_mapping_json)
            .finish()
    }
}
impl PartialEq for DesiredPlanBytes {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl Eq for DesiredPlanBytes {}
impl fmt::Debug for DesiredPlanBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DesiredPlanBytes")
            .field("len", &self.0.len())
            .finish()
    }
}

pub(crate) struct PlanIdentity {
    pub(crate) owner: CorpusOwnerId,
    pub(crate) prefix: CorpusPrefix,
    pub(crate) root: RootIdentity,
    pub(crate) incarnation: CorpusIncarnationId,
    pub(crate) expected: Sequence,
    pub(crate) tx: TransactionId,
    pub(crate) manifest: ManifestDigest,
    pub(crate) prepared: PreparedInputDigest,
    pub(crate) replay_set: ReplaySetDigest,
    pub(crate) generation: Generation,
    pub(crate) quota_charge: u64,
    pub(crate) mappings: Vec<MappingReservationV1>,
    pub(crate) resources: Vec<ResourceKey>,
    pub(crate) data: Vec<ParsedData>,
    pub(crate) catalog: ParsedCatalog,
    pub(crate) graph: ParsedGraph,
    pub(crate) tuples: Vec<ParsedTuple>,
}

pub struct DesiredPublicationPlanV1 {
    pub(crate) bytes: DesiredPlanBytes,
    pub(crate) digest: DesiredPlanDigest,
    pub(crate) identity: PlanIdentity,
}
impl fmt::Debug for DesiredPublicationPlanV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DesiredPublicationPlanV1")
            .field("bytes", &self.bytes)
            .field("digest", &self.digest)
            .field("owner", &self.identity.owner)
            .finish_non_exhaustive()
    }
}
impl DesiredPublicationPlanV1 {
    pub fn parse_canonical_preimage(input: &[u8]) -> Result<Self, ProtocolError> {
        let identity = parse_plan(input)?;
        Ok(Self {
            bytes: DesiredPlanBytes(input.into()),
            digest: DesiredPlanDigest::from_preimage(input),
            identity,
        })
    }
    pub fn canonical_preimage(&self) -> &DesiredPlanBytes {
        &self.bytes
    }
    pub fn digest(&self) -> &DesiredPlanDigest {
        &self.digest
    }
    pub fn owner(&self) -> &CorpusOwnerId {
        &self.identity.owner
    }
    pub fn corpus_incarnation(&self) -> &CorpusIncarnationId {
        &self.identity.incarnation
    }
    pub fn transaction_id(&self) -> &TransactionId {
        &self.identity.tx
    }
    pub fn generation(&self) -> Generation {
        self.identity.generation
    }
    pub fn prepared_input_digest(&self) -> &PreparedInputDigest {
        &self.identity.prepared
    }
    pub fn replay_set_digest(&self) -> &ReplaySetDigest {
        &self.identity.replay_set
    }
    pub fn quota_charge(&self) -> u64 {
        self.identity.quota_charge
    }
    pub fn mapping_reservations(&self) -> &[MappingReservationV1] {
        &self.identity.mappings
    }
    pub fn reserved_resource_keys(&self) -> &[ResourceKey] {
        &self.identity.resources
    }
}

pub struct PlannedCorpusV1 {
    pub(crate) prepared: PreparedCorpusV1,
    pub(crate) desired_plan: DesiredPublicationPlanV1,
    pub(crate) replay_artifacts: Vec<ReplayArtifactV1>,
    pub(crate) physical_names: Vec<PhysicalDataName>,
}
impl fmt::Debug for PlannedCorpusV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlannedCorpusV1")
            .field("desired_plan", &self.desired_plan)
            .field("artifact_count", &self.replay_artifacts.len())
            .finish_non_exhaustive()
    }
}
impl PlannedCorpusV1 {
    pub fn plan(
        prepared: PreparedCorpusV1,
        transition: SequenceTransitionV1,
        generation: Generation,
    ) -> Result<Self, ProtocolError> {
        let mut derived = projection::derive(
            &prepared,
            transition.expected(),
            transition.desired(),
            generation,
        )?;
        let encoded = encode_plan(
            &prepared,
            &derived,
            transition.expected(),
            transition.desired(),
            generation,
        );
        // Fresh construction is not a trusted quota/mapping fast path: the
        // just-produced bytes must pass the same plan parser used by recovery.
        let identity = parse_plan(&encoded)?;
        let desired_plan = DesiredPublicationPlanV1 {
            digest: DesiredPlanDigest::from_preimage(&encoded),
            bytes: DesiredPlanBytes(encoded.into_boxed_slice()),
            identity,
        };
        let physical_names = derived
            .data
            .entries
            .iter()
            .map(|v| v.physical.clone())
            .collect();
        // The future journal repeats artifacts in the desired plan's tuple
        // order. Keep the completed fresh bundle in that exact order too;
        // projection indexes have served their encoding purpose above.
        projection::sort_artifacts_canonically(&mut derived.artifacts);
        Ok(Self {
            prepared,
            desired_plan,
            replay_artifacts: derived.artifacts,
            physical_names,
        })
    }
    pub fn prepared_input(&self) -> &PreparedInputV1 {
        &self.prepared.prepared_input
    }
    pub fn desired_plan(&self) -> &DesiredPublicationPlanV1 {
        &self.desired_plan
    }
    pub fn replay_artifacts(&self) -> &[ReplayArtifactV1] {
        &self.replay_artifacts
    }
    pub fn physical_data_names(&self) -> &[PhysicalDataName] {
        &self.physical_names
    }
}

fn encode_plan(
    prepared: &PreparedCorpusV1,
    derived: &DerivedPlan,
    expected: Sequence,
    desired: Sequence,
    generation: Generation,
) -> Vec<u8> {
    let mut out = Encoder::domain(b"xerj-desired-publication-plan-v1\0");
    out.u32(1);
    out.string(prepared.owner.as_rendered_str());
    out.string(prepared.prefix.as_protocol_str());
    out.string(prepared.root_identity.as_protocol_str());
    out.string(prepared.incarnation.as_rendered_str());
    out.u64(expected.get());
    out.u64(desired.get());
    out.string(derived.tx.as_rendered_str());
    out.string(prepared.manifest.digest().as_rendered_str());
    out.string(prepared.prepared_input.digest().as_rendered_str());
    out.string(derived.replay_set.as_rendered_str());
    out.u64(generation.get());
    out.string(derived.data.projection.as_rendered_str());
    out.array_len(derived.data.entries.len());
    for entry in &derived.data.entries {
        let row = &prepared.data[entry.prepared_index];
        let artifact = &derived.artifacts[entry.artifact_index];
        out.string(row.input.slug.as_protocol_str());
        out.string(row.input.logical_index.as_protocol_str());
        out.string(entry.physical.as_protocol_str());
        out.string(row.input.mapping.digest.as_rendered_str());
        out.u64(row.input.documents.len() as u64);
        out.string(row.id_digest.as_rendered_str());
        out.string(row.content_digest.as_rendered_str());
        out.string(artifact.digest.as_rendered_str());
    }
    let catalog_artifact = &derived.artifacts[derived.catalog.artifact_index];
    out.string(CATALOG_INDEX);
    out.string(derived.catalog.generation_id.as_rendered_str());
    out.string(derived.catalog.incarnation.as_rendered_str());
    out.string(prepared.catalog.input.mapping.digest.as_rendered_str());
    out.u64(prepared.catalog.input.wrappers.len() as u64);
    out.string(prepared.catalog.id_digest.as_rendered_str());
    out.string(prepared.catalog.wrapper_digest.as_rendered_str());
    out.string(derived.catalog.projection.as_rendered_str());
    out.string(catalog_artifact.digest.as_rendered_str());
    let graph = &derived.graph;
    out.string(prepared.graph.input.brain.as_protocol_str());
    out.string(prepared.owner.as_rendered_str());
    out.u64(generation.get());
    out.string(prepared.graph.producer.as_rendered_str());
    out.string(prepared.graph.core_digest.as_rendered_str());
    out.string(graph.token.as_rendered_str());
    out.string(&graph.edges_index);
    out.string(NODE_INDEX);
    out.string(prepared.graph.input.edge_mapping.digest.as_rendered_str());
    out.string(prepared.graph.input.node_mapping.digest.as_rendered_str());
    out.u64(prepared.graph.input.edges.len() as u64);
    out.string(prepared.graph.logical_edge_digest.as_rendered_str());
    out.string(graph.edge_id_set.as_rendered_str());
    out.u64(prepared.graph.input.nodes.len() as u64);
    out.string(prepared.graph.logical_node_digest.as_rendered_str());
    out.string(graph.node_id_set.as_rendered_str());
    out.string(graph.projection.as_rendered_str());
    out.string(
        derived.artifacts[graph.edge_artifact_index]
            .digest
            .as_rendered_str(),
    );
    out.string(
        derived.artifacts[graph.node_artifact_index]
            .digest
            .as_rendered_str(),
    );
    out.array_len(derived.mapping_reservations.len());
    for reservation in &derived.mapping_reservations {
        encode_mapping_reservation(&mut out, reservation);
    }
    out.u64(derived.quota_charge);
    out.array_len(derived.resource_keys.len());
    for key in &derived.resource_keys {
        out.string(key.as_protocol_str());
    }
    let order = projection::sorted_artifact_indices(&derived.artifacts);
    out.array_len(order.len());
    for index in order {
        projection::encode_replay_tuple(&mut out, &derived.artifacts[index]);
    }
    out.finish()
}

pub(crate) fn encode_mapping_reservation(out: &mut Encoder, reservation: &MappingReservationV1) {
    out.string(reservation.projection_kind.protocol_str());
    out.string(reservation.resource_key.as_protocol_str());
    out.string(reservation.mapping_digest.as_rendered_str());
    out.bytes(reservation.canonical_mapping_json.canonical_json());
}

fn validate_canonical_mapping(input: &[u8], expected_digest: &MappingDigest) -> Result<()> {
    let value = canonical_json::parse(input, "mapping reservation canonical JSON")?;
    canonical_json::object(&value, "mapping reservation canonical JSON")?;
    if canonical_json::canonicalize(&value) != input {
        return Err(error(
            ProtocolErrorKind::NonCanonicalEncoding,
            "mapping reservation JSON is not RFC 8785 canonical",
        ));
    }
    let mut preimage = Encoder::domain(b"xerj-mapping-v1\0");
    preimage.bytes(input);
    if MappingDigest::from_preimage(&preimage.finish()) != *expected_digest {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "mapping reservation digest does not recompute",
        ));
    }
    Ok(())
}

fn checked_usize_len(value: usize, field: &str) -> Result<u64> {
    u64::try_from(value).map_err(|_| {
        error(
            ProtocolErrorKind::ArithmeticOverflow,
            format_args!("{field} length exceeds u64"),
        )
    })
}

fn mapping_reservation_byte_length(reservation: &MappingReservationV1) -> Result<u64> {
    crate::codec::checked_add(
        [
            8,
            checked_usize_len(
                reservation.projection_kind.protocol_str().len(),
                "mapping projection",
            )?,
            8,
            checked_usize_len(
                reservation.resource_key.as_protocol_str().len(),
                "mapping resource",
            )?,
            8,
            checked_usize_len(
                reservation.mapping_digest.as_rendered_str().len(),
                "mapping digest",
            )?,
            8,
            checked_usize_len(
                reservation.canonical_mapping_json.canonical_json().len(),
                "mapping JSON",
            )?,
        ],
        "mapping record",
    )
}

pub(crate) fn compute_quota_charge(
    mappings: &[MappingReservationV1],
    artifact_lengths: impl IntoIterator<Item = u64>,
    operation_counts: impl IntoIterator<Item = u64>,
    resource_count: usize,
) -> Result<u64> {
    let mapping_charge = mappings.iter().try_fold(0u64, |charge, reservation| {
        charge
            .checked_add(mapping_reservation_byte_length(reservation)?)
            .ok_or_else(|| {
                error(
                    ProtocolErrorKind::ArithmeticOverflow,
                    "mapping charge addition overflow",
                )
            })
    })?;
    let artifact_charge = crate::codec::checked_add(artifact_lengths, "artifact charge")?;
    let operation_count = crate::codec::checked_add(operation_counts, "operation count")?;
    let operation_charge = crate::codec::checked_mul(64, operation_count, "operation charge")?;
    let resource_count = checked_usize_len(resource_count, "resource count")?;
    let resource_charge = crate::codec::checked_mul(4096, resource_count, "resource charge")?;
    crate::codec::checked_add(
        [
            mapping_charge,
            artifact_charge,
            operation_charge,
            resource_charge,
        ],
        "stage charge",
    )
}

fn sorted_mapping_resource_keys(mappings: &[MappingReservationV1]) -> Vec<ResourceKey> {
    let mut resources: Vec<_> = mappings
        .iter()
        .map(|mapping| mapping.resource_key.clone())
        .collect();
    resources.sort_by(|left, right| left.as_protocol_str().cmp(right.as_protocol_str()));
    resources
}

pub(crate) struct ParsedData {
    pub(crate) slug: DataSlug,
    pub(crate) logical: LogicalIndexName,
    pub(crate) physical: PhysicalDataName,
    pub(crate) mapping: MappingDigest,
    pub(crate) count: u64,
    pub(crate) ids: DataIdDigest,
    pub(crate) content: DataContentDigest,
    pub(crate) artifact: ReplayArtifactDigest,
}
pub(crate) struct ParsedCatalog {
    pub(crate) storage: Box<str>,
    pub(crate) generation_id: GenerationId,
    pub(crate) incarnation: CatalogGenerationIncarnationId,
    pub(crate) mapping: MappingDigest,
    pub(crate) count: u64,
    pub(crate) ids: CatalogIdDigest,
    pub(crate) content: CatalogWrapperDigest,
    pub(crate) projection: CatalogProjectionDigest,
    pub(crate) artifact: ReplayArtifactDigest,
}
pub(crate) struct ParsedGraph {
    pub(crate) brain: BrainName,
    pub(crate) owner: CorpusOwnerId,
    pub(crate) generation: Generation,
    pub(crate) producer: ProducerId,
    pub(crate) core: GraphCoreDigest,
    pub(crate) token: GraphToken,
    pub(crate) edges_index: Box<str>,
    pub(crate) nodes_index: Box<str>,
    pub(crate) edge_mapping: MappingDigest,
    pub(crate) node_mapping: MappingDigest,
    pub(crate) edge_count: u64,
    pub(crate) logical_edges: LogicalEdgeSetDigest,
    pub(crate) edge_ids: EdgePhysicalIdSetDigest,
    pub(crate) node_count: u64,
    pub(crate) logical_nodes: LogicalNodeSetDigest,
    pub(crate) node_ids: NodePhysicalIdSetDigest,
    pub(crate) projection: GraphProjectionDigest,
    pub(crate) edge_artifact: ReplayArtifactDigest,
    pub(crate) node_artifact: ReplayArtifactDigest,
}
pub(crate) struct ParsedTuple {
    pub(crate) kind: ReplayArtifactKind,
    pub(crate) projection: ProjectionKind,
    pub(crate) resource: ResourceKey,
    pub(crate) length: u64,
    pub(crate) count: u64,
    pub(crate) digest: ReplayArtifactDigest,
}

pub(crate) fn parse_plan(input: &[u8]) -> Result<PlanIdentity> {
    let mut c = Cursor::new(input);
    c.domain(b"xerj-desired-publication-plan-v1\0")?;
    if c.u32("format_version")? != 1 {
        return Err(error(
            ProtocolErrorKind::InvalidVersion,
            "desired plan version must equal 1",
        ));
    }
    let owner: CorpusOwnerId = c.string("owner")?.parse()?;
    let prefix = CorpusPrefix::from_str(c.string("prefix")?)?;
    let root = RootIdentity::from_str(c.string("root")?)?;
    let incarnation: CorpusIncarnationId = c.string("incarnation")?.parse()?;
    if identity::owner(&root, &prefix) != owner {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "desired plan owner does not match root/prefix",
        ));
    }
    let expected = Sequence::new(c.u64("expected_sequence")?);
    let desired = Sequence::new(c.u64("desired_sequence")?);
    SequenceTransitionV1::new(expected, desired)?;
    let tx: TransactionId = c.string("tx")?.parse()?;
    let manifest: ManifestDigest = c.string("manifest")?.parse()?;
    let prepared: PreparedInputDigest = c.string("prepared")?.parse()?;
    let replay_set: ReplaySetDigest = c.string("replay_set")?.parse()?;
    if identity::transaction(
        &owner,
        &incarnation,
        expected,
        desired,
        &manifest,
        &prepared,
    ) != tx
    {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "desired plan transaction does not recompute",
        ));
    }
    let generation = Generation::new(c.u64("data.generation")?);
    let data_projection: DataProjectionDigest = c.string("data.projection")?.parse()?;
    let data_len = c.len("data.indices")?;
    let mut data = Vec::with_capacity(data_len);
    let mut last = None;
    for _ in 0..data_len {
        let slug = DataSlug::from_str(c.string("data.slug")?)?;
        let logical = LogicalIndexName::from_str(c.string("data.logical")?)?;
        let physical = PhysicalDataName::from_str(c.string("data.physical")?)?;
        let mapping = c.string("data.mapping")?.parse()?;
        let count = c.u64("data.count")?;
        let ids = c.string("data.ids")?.parse()?;
        let content = c.string("data.content")?.parse()?;
        let artifact = c.string("data.artifact")?.parse()?;
        if last
            .as_deref()
            .is_some_and(|v: &str| v >= slug.as_protocol_str())
        {
            return Err(error(
                ProtocolErrorKind::NonCanonicalEncoding,
                "data entries are not strictly sorted",
            ));
        }
        last = Some(slug.as_protocol_str().to_owned());
        if logical.as_protocol_str()
            != format!("{}-{}", prefix.as_protocol_str(), slug.as_protocol_str())
        {
            return Err(error(
                ProtocolErrorKind::CrossFieldMismatch,
                "data logical route does not match prefix/slug",
            ));
        }
        if identity::physical_data_name(&owner, &incarnation, &tx, &manifest, generation, &slug)?
            != physical
        {
            return Err(error(
                ProtocolErrorKind::CrossFieldMismatch,
                "physical data name does not recompute",
            ));
        }
        data.push(ParsedData {
            slug,
            logical,
            physical,
            mapping,
            count,
            ids,
            content,
            artifact,
        });
    }
    let mut dp = Encoder::domain(b"xerj-data-projection-v1\0");
    dp.u64(generation.get());
    dp.array_len(data.len());
    for v in &data {
        dp.string(v.slug.as_protocol_str());
        dp.string(v.logical.as_protocol_str());
        dp.string(v.physical.as_protocol_str());
        dp.string(v.mapping.as_rendered_str());
        dp.u64(v.count);
        dp.string(v.ids.as_rendered_str());
        dp.string(v.content.as_rendered_str());
    }
    if DataProjectionDigest::from_preimage(&dp.finish()) != data_projection {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "data projection does not recompute",
        ));
    }
    let catalog = ParsedCatalog {
        storage: c.string("catalog.storage")?.into(),
        generation_id: c.string("catalog.generation")?.parse()?,
        incarnation: c.string("catalog.incarnation")?.parse()?,
        mapping: c.string("catalog.mapping")?.parse()?,
        count: c.u64("catalog.count")?,
        ids: c.string("catalog.ids")?.parse()?,
        content: c.string("catalog.content")?.parse()?,
        projection: c.string("catalog.projection")?.parse()?,
        artifact: c.string("catalog.artifact")?.parse()?,
    };
    if &*catalog.storage != CATALOG_INDEX
        || identity::generation_id(&owner, &incarnation, generation, &tx) != catalog.generation_id
    {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "catalog target identity does not recompute",
        ));
    }
    let mut cp = Encoder::domain(b"xerj-catalog-projection-v1\0");
    cp.string(owner.as_rendered_str());
    cp.string(incarnation.as_rendered_str());
    cp.u64(generation.get());
    cp.string(catalog.generation_id.as_rendered_str());
    cp.u64(catalog.count);
    cp.string(catalog.content.as_rendered_str());
    if CatalogProjectionDigest::from_preimage(&cp.finish()) != catalog.projection
        || identity::catalog_incarnation(&owner, &incarnation, generation, &tx, &catalog.projection)
            != catalog.incarnation
    {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "catalog projection/incarnation does not recompute",
        ));
    }
    let graph = ParsedGraph {
        brain: BrainName::from_str(c.string("graph.brain")?)?,
        owner: c.string("graph.owner")?.parse()?,
        generation: Generation::new(c.u64("graph.generation")?),
        producer: c.string("graph.producer")?.parse()?,
        core: c.string("graph.core")?.parse()?,
        token: c.string("graph.token")?.parse()?,
        edges_index: c.string("graph.edges_index")?.into(),
        nodes_index: c.string("graph.nodes_index")?.into(),
        edge_mapping: c.string("graph.edge_mapping")?.parse()?,
        node_mapping: c.string("graph.node_mapping")?.parse()?,
        edge_count: c.u64("graph.edge_count")?,
        logical_edges: c.string("graph.logical_edges")?.parse()?,
        edge_ids: c.string("graph.edge_ids")?.parse()?,
        node_count: c.u64("graph.node_count")?,
        logical_nodes: c.string("graph.logical_nodes")?.parse()?,
        node_ids: c.string("graph.node_ids")?.parse()?,
        projection: c.string("graph.projection")?.parse()?,
        edge_artifact: c.string("graph.edge_artifact")?.parse()?,
        node_artifact: c.string("graph.node_artifact")?.parse()?,
    };
    if graph.owner != owner
        || graph.generation != generation
        || graph.brain.as_protocol_str() != prefix.as_protocol_str()
        || *graph.edges_index != format!(".xerj-memory-{}-edges", graph.brain.as_protocol_str())
        || &*graph.nodes_index != NODE_INDEX
    {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "graph target join mismatch",
        ));
    }
    let mut gc = Encoder::domain(b"xerj-graph-projection-core-v1\0");
    crate::prepared::encode_graph_core_body(
        &mut gc,
        graph.brain.as_protocol_str(),
        &owner,
        &graph.producer,
        graph.edge_count,
        &graph.logical_edges,
        graph.node_count,
        &graph.logical_nodes,
    );
    if GraphCoreDigest::from_preimage(&gc.finish()) != graph.core
        || identity::graph_token(&owner, &incarnation, generation, &tx, &graph.core) != graph.token
    {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "graph core/token does not recompute",
        ));
    }
    let mut gp = Encoder::domain(b"xerj-graph-projection-v1\0");
    crate::prepared::encode_graph_core_body(
        &mut gp,
        graph.brain.as_protocol_str(),
        &owner,
        &graph.producer,
        graph.edge_count,
        &graph.logical_edges,
        graph.node_count,
        &graph.logical_nodes,
    );
    gp.string(graph.core.as_rendered_str());
    gp.u64(generation.get());
    gp.string(graph.token.as_rendered_str());
    gp.string(graph.edge_ids.as_rendered_str());
    gp.string(graph.node_ids.as_rendered_str());
    if GraphProjectionDigest::from_preimage(&gp.finish()) != graph.projection {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "graph projection does not recompute",
        ));
    }
    let mapping_len = c.len("mapping reservations")?;
    let mut mappings = Vec::with_capacity(mapping_len);
    let mut last_mapping = None;
    for _ in 0..mapping_len {
        let projection_kind: ProjectionKind = c.string("mapping.projection")?.parse()?;
        let resource_key = ResourceKey::from_str(c.string("mapping.resource")?)?;
        let mapping_digest: MappingDigest = c.string("mapping.digest")?.parse()?;
        let json_len = c.len("mapping.canonical_json")?;
        let canonical_mapping_json = c.take(json_len)?.to_vec().into_boxed_slice();
        let key = (
            projection_kind.protocol_str().to_owned(),
            resource_key.as_protocol_str().to_owned(),
        );
        if last_mapping
            .as_ref()
            .is_some_and(|previous| previous >= &key)
        {
            return Err(error(
                ProtocolErrorKind::NonCanonicalEncoding,
                "mapping reservations are not strictly sorted",
            ));
        }
        last_mapping = Some(key);
        mappings.push(MappingReservationV1::from_canonical_mapping(
            projection_kind,
            resource_key,
            mapping_digest,
            canonical_mapping_json,
        )?);
    }
    let quota_charge = c.u64("quota_charge")?;
    let resource_len = c.len("resources")?;
    let mut resources = Vec::with_capacity(resource_len);
    let mut last_resource = None;
    for _ in 0..resource_len {
        let key = ResourceKey::from_str(c.string("resource")?)?;
        if last_resource
            .as_deref()
            .is_some_and(|v: &str| v >= key.as_protocol_str())
        {
            return Err(error(
                ProtocolErrorKind::NonCanonicalEncoding,
                "resource keys are not strictly sorted",
            ));
        }
        last_resource = Some(key.as_protocol_str().to_owned());
        resources.push(key);
    }
    let tuple_len = c.len("replay tuples")?;
    let mut tuples = Vec::with_capacity(tuple_len);
    let mut last_tuple = None;
    for _ in 0..tuple_len {
        let tuple = ParsedTuple {
            kind: c.string("tuple.kind")?.parse()?,
            projection: c.string("tuple.projection")?.parse()?,
            resource: ResourceKey::from_str(c.string("tuple.resource")?)?,
            length: c.u64("tuple.length")?,
            count: c.u64("tuple.count")?,
            digest: c.string("tuple.digest")?.parse()?,
        };
        let key = (
            tuple.projection.protocol_str().to_owned(),
            tuple.resource.as_protocol_str().to_owned(),
            tuple.kind.protocol_str().to_owned(),
            tuple.digest.as_rendered_str().to_owned(),
        );
        if last_tuple.as_ref().is_some_and(|v| v >= &key) {
            return Err(error(
                ProtocolErrorKind::NonCanonicalEncoding,
                "replay tuples are not strictly sorted",
            ));
        }
        last_tuple = Some(key);
        validate_tuple_kind(&tuple)?;
        tuples.push(tuple);
    }
    c.finish()?;
    let mut rs = Encoder::domain(b"xerj-replay-set-v1\0");
    rs.array_len(tuples.len());
    for t in &tuples {
        rs.string(t.kind.protocol_str());
        rs.string(t.projection.protocol_str());
        rs.string(t.resource.as_protocol_str());
        rs.u64(t.length);
        rs.u64(t.count);
        rs.string(t.digest.as_rendered_str());
    }
    if ReplaySetDigest::from_preimage(&rs.finish()) != replay_set {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "replay set does not recompute",
        ));
    }
    let tuple_resources: Vec<_> = tuples
        .iter()
        .map(|v| v.resource.as_protocol_str())
        .collect();
    if resources
        .iter()
        .map(|v| v.as_protocol_str())
        .collect::<Vec<_>>()
        != {
            let mut v = tuple_resources.clone();
            v.sort_unstable();
            v
        }
    {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "reserved resources do not match replay resources",
        ));
    }
    let required_tuple_count = data.len().checked_add(3).ok_or_else(|| {
        error(
            ProtocolErrorKind::ArithmeticOverflow,
            "declared replay tuple cardinality overflow",
        )
    })?;
    if tuples.len() != required_tuple_count {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "replay tuple cardinality does not match declared projections",
        ));
    }
    if mappings.len() != required_tuple_count || resources.len() != required_tuple_count {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "mapping/resource/replay cardinalities do not match declared projections",
        ));
    }
    for row in &data {
        let resource = format!("data/{}", row.physical.as_protocol_str());
        require_mapping(&mappings, ProjectionKind::Data, &resource, &row.mapping)?;
        require_tuple(
            &tuples,
            ProjectionKind::Data,
            &resource,
            &row.artifact,
            row.count,
        )?;
    }
    let catalog_resource = format!(
        "catalog/{CATALOG_INDEX}/{}",
        catalog.generation_id.as_rendered_str()
    );
    require_mapping(
        &mappings,
        ProjectionKind::Catalog,
        &catalog_resource,
        &catalog.mapping,
    )?;
    require_tuple(
        &tuples,
        ProjectionKind::Catalog,
        &catalog_resource,
        &catalog.artifact,
        catalog.count,
    )?;
    let graph_edge_resource = format!(
        "graph-edge/{}/{}",
        graph.edges_index,
        graph.token.as_rendered_str()
    );
    require_mapping(
        &mappings,
        ProjectionKind::GraphEdge,
        &graph_edge_resource,
        &graph.edge_mapping,
    )?;
    require_tuple(
        &tuples,
        ProjectionKind::GraphEdge,
        &graph_edge_resource,
        &graph.edge_artifact,
        graph.edge_count,
    )?;
    let graph_node_resource = format!("graph-node/{NODE_INDEX}/{}", graph.token.as_rendered_str());
    require_mapping(
        &mappings,
        ProjectionKind::GraphNode,
        &graph_node_resource,
        &graph.node_mapping,
    )?;
    require_tuple(
        &tuples,
        ProjectionKind::GraphNode,
        &graph_node_resource,
        &graph.node_artifact,
        graph.node_count,
    )?;
    let mapping_resources = sorted_mapping_resource_keys(&mappings);
    if resources
        .iter()
        .map(ResourceKey::as_protocol_str)
        .ne(mapping_resources.iter().map(ResourceKey::as_protocol_str))
    {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "reserved resources do not match mapping reservations",
        ));
    }
    let recomputed_quota = compute_quota_charge(
        &mappings,
        tuples.iter().map(|tuple| tuple.length),
        tuples.iter().map(|tuple| tuple.count),
        resources.len(),
    )?;
    if quota_charge != recomputed_quota {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "quota charge does not recompute from persisted plan bytes",
        ));
    }
    Ok(PlanIdentity {
        owner,
        prefix,
        root,
        incarnation,
        expected,
        tx,
        manifest,
        prepared,
        replay_set,
        generation,
        quota_charge,
        mappings,
        resources,
        data,
        catalog,
        graph,
        tuples,
    })
}

fn validate_tuple_kind(tuple: &ParsedTuple) -> Result<()> {
    let ok = matches!(
        (tuple.kind, tuple.projection),
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
    if !ok {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "artifact kind and projection kind mismatch",
        ));
    }
    Ok(())
}

fn require_mapping(
    mappings: &[MappingReservationV1],
    projection: ProjectionKind,
    resource: &str,
    digest: &MappingDigest,
) -> Result<()> {
    let mut matches = mappings.iter().filter(|mapping| {
        mapping.projection_kind == projection && mapping.resource_key.as_protocol_str() == resource
    });
    let Some(mapping) = matches.next() else {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "plan projection does not join exactly one mapping reservation",
        ));
    };
    if matches.next().is_some() || &mapping.mapping_digest != digest {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "plan projection does not join exactly one mapping reservation",
        ));
    }
    Ok(())
}

fn require_tuple(
    tuples: &[ParsedTuple],
    projection: ProjectionKind,
    resource: &str,
    digest: &ReplayArtifactDigest,
    expected_count: u64,
) -> Result<()> {
    let mut matches = tuples.iter().filter(|v| {
        v.projection == projection
            && v.resource.as_protocol_str() == resource
            && &v.digest == digest
    });
    let Some(tuple) = matches.next() else {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "plan projection does not join exactly one replay tuple",
        ));
    };
    if matches.next().is_some() {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "plan projection does not join exactly one replay tuple",
        ));
    }
    if tuple.count != expected_count {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "replay tuple operation count does not match declared projection count",
        ));
    }
    Ok(())
}
