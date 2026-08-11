use crate::{
    canonical_json,
    codec::Encoder,
    digest::{
        DesiredPlanDigest, ExpectedPublicationDigest, PreparedInputDigest, ReplaySetDigest,
        SyncBeginDigest,
    },
    error::{error, ProtocolError, ProtocolErrorKind, Result},
    plan::{DesiredPublicationPlanV1, ParsedTuple, PlannedCorpusV1},
    prepared::{PreparedCorpusV1, PreparedInputSummaryV1, PreparedInputV1},
    publication::ExpectedPublicationV1,
    replay::{
        validate_replay_artifact_bytes_v1, ProjectionKind, ReplayArtifactV1, ReplayEvidenceV1,
        ReplayTupleExpectationV1,
    },
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::fmt;

const MAX_PLAN_BYTES: usize = 16 * 1024 * 1024;

macro_rules! persisted_bytes {
    ($name:ident) => {
        pub struct $name(Box<[u8]>);
        impl $name {
            pub fn from_journal(bytes: Box<[u8]>) -> Self {
                Self(bytes)
            }
        }
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct(stringify!($name))
                    .field("len", &self.0.len())
                    .finish()
            }
        }
    };
}

persisted_bytes!(PersistedPreparedInputBytesV1);
persisted_bytes!(PersistedReplayArtifactBytesV1);
persisted_bytes!(PersistedDesiredPlanBytesV1);
persisted_bytes!(PersistedSyncBeginBytesV1);

pub struct SyncBeginJsonBytes(Box<[u8]>);
impl SyncBeginJsonBytes {
    pub fn canonical_json(&self) -> &[u8] {
        &self.0
    }
}
impl PartialEq for SyncBeginJsonBytes {
    fn eq(&self, o: &Self) -> bool {
        self.0 == o.0
    }
}
impl Eq for SyncBeginJsonBytes {}
impl fmt::Debug for SyncBeginJsonBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SyncBeginJsonBytes")
            .field("len", &self.0.len())
            .finish()
    }
}

pub struct SyncBeginV1 {
    canonical: SyncBeginJsonBytes,
    expected: ExpectedPublicationV1,
    plan: DesiredPublicationPlanV1,
    digest: SyncBeginDigest,
}
impl fmt::Debug for SyncBeginV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SyncBeginV1")
            .field("canonical", &self.canonical)
            .field("digest", &self.digest)
            .finish_non_exhaustive()
    }
}
impl SyncBeginV1 {
    fn from_parts(expected: ExpectedPublicationV1, plan: DesiredPublicationPlanV1) -> Self {
        let digest = compute_digest(&expected, &plan);
        let canonical = render_json(&expected, &plan, &digest);
        Self {
            canonical: SyncBeginJsonBytes(canonical.into_boxed_slice()),
            expected,
            plan,
            digest,
        }
    }

    pub fn parse_closed_json(input: &[u8]) -> Result<Self, ProtocolError> {
        let value = canonical_json::parse(input, "sync_begin")?;
        let f = canonical_json::closed(
            &value,
            "sync_begin",
            &[
                "format_version",
                "expected_publication",
                "expected_publication_digest",
                "canonical_plan_bytes",
                "plan_digest",
                "prepared_input_digest",
                "replay_set_digest",
                "sync_begin_digest",
            ],
        )?;
        if canonical_json::u64(f[0], "sync_begin.format_version")? != 1 {
            return Err(error(
                ProtocolErrorKind::InvalidVersion,
                "sync begin version must equal 1",
            ));
        }
        let expected_json = canonical_json::canonicalize(f[1]);
        let expected = ExpectedPublicationV1::parse_closed_json(&expected_json)?;
        let expected_digest: ExpectedPublicationDigest =
            canonical_json::string(f[2], "sync_begin.expected_publication_digest")?.parse()?;
        if expected.digest() != &expected_digest {
            return Err(error(
                ProtocolErrorKind::CrossFieldMismatch,
                "expected publication digest mismatch",
            ));
        }
        let encoded = canonical_json::string(f[3], "sync_begin.canonical_plan_bytes")?;
        let plan_bytes = STANDARD.decode(encoded).map_err(|_| {
            error(
                ProtocolErrorKind::NonCanonicalEncoding,
                "canonical_plan_bytes is not padded standard base64",
            )
        })?;
        if plan_bytes.len() > MAX_PLAN_BYTES {
            return Err(error(
                ProtocolErrorKind::BoundsExceeded,
                "canonical plan exceeds 16 MiB",
            ));
        }
        if STANDARD.encode(&plan_bytes) != encoded {
            return Err(error(
                ProtocolErrorKind::NonCanonicalEncoding,
                "canonical_plan_bytes base64 does not re-encode exactly",
            ));
        }
        let plan = DesiredPublicationPlanV1::parse_canonical_preimage(&plan_bytes)?;
        let plan_digest: DesiredPlanDigest =
            canonical_json::string(f[4], "sync_begin.plan_digest")?.parse()?;
        let prepared: PreparedInputDigest =
            canonical_json::string(f[5], "sync_begin.prepared_input_digest")?.parse()?;
        let replay: ReplaySetDigest =
            canonical_json::string(f[6], "sync_begin.replay_set_digest")?.parse()?;
        if plan.digest() != &plan_digest
            || plan.prepared_input_digest() != &prepared
            || plan.replay_set_digest() != &replay
        {
            return Err(error(
                ProtocolErrorKind::CrossFieldMismatch,
                "sync begin nested plan digests mismatch",
            ));
        }
        validate_expected(&expected, &plan)?;
        let digest = compute_digest(&expected, &plan);
        let attached: SyncBeginDigest =
            canonical_json::string(f[7], "sync_begin.sync_begin_digest")?.parse()?;
        if digest != attached {
            return Err(error(
                ProtocolErrorKind::CrossFieldMismatch,
                "sync begin digest mismatch",
            ));
        }
        let canonical = render_json(&expected, &plan, &digest);
        if canonical != input {
            return Err(error(
                ProtocolErrorKind::NonCanonicalEncoding,
                "sync begin JSON is not the exact canonical encoding",
            ));
        }
        Ok(Self {
            canonical: SyncBeginJsonBytes(canonical.into_boxed_slice()),
            expected,
            plan,
            digest,
        })
    }
    pub fn canonical_json(&self) -> &SyncBeginJsonBytes {
        &self.canonical
    }
    pub fn expected_publication(&self) -> &ExpectedPublicationV1 {
        &self.expected
    }
    pub fn desired_plan(&self) -> &DesiredPublicationPlanV1 {
        &self.plan
    }
    pub fn plan_digest(&self) -> &DesiredPlanDigest {
        self.plan.digest()
    }
    pub fn prepared_input_digest(&self) -> &PreparedInputDigest {
        self.plan.prepared_input_digest()
    }
    pub fn replay_set_digest(&self) -> &ReplaySetDigest {
        self.plan.replay_set_digest()
    }
    pub fn digest(&self) -> &SyncBeginDigest {
        &self.digest
    }
}

pub struct DurableBeginBundleV1 {
    sync_begin: SyncBeginV1,
    prepared_input: PreparedInputV1,
    replay_artifacts: Vec<ReplayArtifactV1>,
}

struct BundlePartsV1 {
    sync_begin: SyncBeginV1,
    prepared_input: PreparedInputV1,
    replay_artifacts: Vec<ReplayArtifactV1>,
}
impl fmt::Debug for DurableBeginBundleV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DurableBeginBundleV1")
            .field("sync_begin", &self.sync_begin)
            .field("prepared_input", &self.prepared_input)
            .field("artifact_count", &self.replay_artifacts.len())
            .finish_non_exhaustive()
    }
}
impl DurableBeginBundleV1 {
    pub fn build(
        expected: ExpectedPublicationV1,
        planned: PlannedCorpusV1,
    ) -> Result<Self, ProtocolError> {
        let PlannedCorpusV1 {
            prepared,
            desired_plan,
            replay_artifacts,
            physical_names: _,
        } = planned;
        let PreparedCorpusV1 { prepared_input, .. } = prepared;
        validate_bundle_parts_v1(BundlePartsV1 {
            sync_begin: SyncBeginV1::from_parts(expected, desired_plan),
            prepared_input,
            replay_artifacts,
        })
    }
    pub fn rehydrate(
        prepared_input: PersistedPreparedInputBytesV1,
        replay_artifacts: Vec<PersistedReplayArtifactBytesV1>,
        desired_plan: PersistedDesiredPlanBytesV1,
        sync_begin: PersistedSyncBeginBytesV1,
    ) -> Result<Self, ProtocolError> {
        let prepared_input = PreparedInputV1::parse_canonical_preimage(&prepared_input.0)?;
        let desired_plan = DesiredPublicationPlanV1::parse_canonical_preimage(&desired_plan.0)?;
        let mut sync_begin = SyncBeginV1::parse_closed_json(&sync_begin.0)?;
        if replay_artifacts.len() != desired_plan.identity.tuples.len() {
            return Err(error(
                ProtocolErrorKind::CrossFieldMismatch,
                "persisted replay artifact cardinality differs from desired-plan tuples",
            ));
        }
        let replay_artifacts = desired_plan
            .identity
            .tuples
            .iter()
            .zip(replay_artifacts)
            .map(|(tuple, bytes)| {
                ReplayArtifactV1::from_persisted_tuple(
                    tuple.kind,
                    tuple.projection,
                    tuple.resource.clone(),
                    tuple.count,
                    tuple.digest.clone(),
                    bytes.0,
                )
            })
            .collect();
        sync_begin.plan = desired_plan;
        validate_bundle_parts_v1(BundlePartsV1 {
            sync_begin,
            prepared_input,
            replay_artifacts,
        })
    }
    pub fn sync_begin(&self) -> &SyncBeginV1 {
        &self.sync_begin
    }
    pub fn prepared_input(&self) -> &PreparedInputV1 {
        &self.prepared_input
    }
    pub fn desired_plan(&self) -> &DesiredPublicationPlanV1 {
        &self.sync_begin.plan
    }
    pub fn replay_artifacts(&self) -> &[ReplayArtifactV1] {
        &self.replay_artifacts
    }
}

fn validate_bundle_parts_v1(mut parts: BundlePartsV1) -> Result<DurableBeginBundleV1> {
    let prepared_input_bytes = parts
        .prepared_input
        .canonical_preimage()
        .canonical_preimage();
    let desired_plan_bytes = parts
        .sync_begin
        .desired_plan()
        .canonical_preimage()
        .canonical_preimage();
    let sync_begin_bytes = parts.sync_begin.canonical_json().canonical_json();

    let prepared_summary = crate::prepared::parse_prepared_encoding(prepared_input_bytes)?;
    let desired_identity = crate::plan::parse_plan(desired_plan_bytes)?;
    let reparsed_sync_begin = SyncBeginV1::parse_closed_json(sync_begin_bytes)?;

    if reparsed_sync_begin
        .desired_plan()
        .canonical_preimage()
        .canonical_preimage()
        != desired_plan_bytes
    {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "standalone desired-plan bytes differ from sync begin embedded plan bytes",
        ));
    }
    if parts.prepared_input.digest()
        != &crate::digest::PreparedInputDigest::from_preimage(prepared_input_bytes)
        || parts.sync_begin.desired_plan().digest()
            != &crate::digest::DesiredPlanDigest::from_preimage(desired_plan_bytes)
        || parts.sync_begin.digest() != reparsed_sync_begin.digest()
        || parts
            .sync_begin
            .expected_publication()
            .canonical_json()
            .canonical_json()
            != reparsed_sync_begin
                .expected_publication()
                .canonical_json()
                .canonical_json()
        || parts.prepared_input.digest() != parts.sync_begin.desired_plan().prepared_input_digest()
        || parts.prepared_input.digest() != reparsed_sync_begin.prepared_input_digest()
        || parts.sync_begin.desired_plan().digest() != reparsed_sync_begin.plan_digest()
        || parts.sync_begin.desired_plan().replay_set_digest()
            != reparsed_sync_begin.replay_set_digest()
    {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "prepared/plan/begin digest join mismatch",
        ));
    }
    parts.prepared_input.summary = prepared_summary;
    parts.sync_begin.plan.identity = desired_identity;
    validate_expected(
        parts.sync_begin.expected_publication(),
        parts.sync_begin.desired_plan(),
    )?;
    validate_prepared_plan_join(
        &parts.prepared_input.summary,
        parts.sync_begin.desired_plan(),
    )?;

    if parts.replay_artifacts.len() != parts.sync_begin.plan.identity.tuples.len() {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "persisted replay artifact cardinality differs from desired-plan tuples",
        ));
    }
    for (tuple, artifact) in parts
        .sync_begin
        .plan
        .identity
        .tuples
        .iter()
        .zip(&parts.replay_artifacts)
    {
        if artifact.kind() != tuple.kind
            || artifact.projection_kind() != tuple.projection
            || artifact.resource_key() != &tuple.resource
            || artifact.byte_length() != tuple.length
            || artifact.operation_count() != tuple.count
            || artifact.digest() != &tuple.digest
        {
            return Err(error(
                ProtocolErrorKind::CrossFieldMismatch,
                "retained replay artifact metadata differs from desired-plan tuple",
            ));
        }
        let target = tuple_target(parts.sync_begin.desired_plan(), tuple)?;
        let expectation = ReplayTupleExpectationV1 {
            kind: tuple.kind,
            projection_kind: tuple.projection,
            resource_key: &tuple.resource,
            target,
            byte_length: tuple.length,
            operation_count: tuple.count,
            digest: &tuple.digest,
            owner: &parts.sync_begin.plan.identity.owner,
            corpus_incarnation: &parts.sync_begin.plan.identity.incarnation,
            generation: parts.sync_begin.plan.identity.generation,
            transaction: &parts.sync_begin.plan.identity.tx,
            graph_producer: &parts.sync_begin.plan.identity.graph.producer,
        };
        let evidence =
            validate_replay_artifact_bytes_v1(&expectation, artifact.bytes().artifact_bytes())?;
        validate_replay_evidence(
            &parts.prepared_input.summary,
            parts.sync_begin.desired_plan(),
            tuple,
            &evidence,
        )?;
    }
    if crate::projection::compute_replay_set(&parts.replay_artifacts)
        != *parts.sync_begin.desired_plan().replay_set_digest()
    {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "validated replay artifacts do not recompute the desired replay set",
        ));
    }

    // This is intentionally the sole `DurableBeginBundleV1` struct literal.
    // Both fresh build and persisted recovery arrive here from complete bytes.
    Ok(DurableBeginBundleV1 {
        sync_begin: parts.sync_begin,
        prepared_input: parts.prepared_input,
        replay_artifacts: parts.replay_artifacts,
    })
}

fn validate_prepared_plan_join(
    prepared: &PreparedInputSummaryV1,
    plan: &DesiredPublicationPlanV1,
) -> Result<()> {
    let identity = &plan.identity;
    if prepared.owner != identity.owner
        || prepared.incarnation != identity.incarnation
        || prepared.manifest != identity.manifest
        || prepared.data.len() != identity.data.len()
    {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "prepared input identity/data cardinality differs from desired plan",
        ));
    }
    for (prepared, planned) in prepared.data.iter().zip(&identity.data) {
        if prepared.slug != planned.slug
            || prepared.mapping != planned.mapping
            || prepared.count != planned.count
            || prepared.ids != planned.ids
            || prepared.content != planned.content
        {
            return Err(error(
                ProtocolErrorKind::CrossFieldMismatch,
                "prepared data summary differs from desired plan",
            ));
        }
    }
    if prepared.catalog.count != identity.catalog.count
        || prepared.catalog.ids != identity.catalog.ids
        || prepared.catalog.content != identity.catalog.content
    {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "prepared catalog summary differs from desired plan",
        ));
    }
    let prepared_graph = &prepared.graph;
    let planned_graph = &identity.graph;
    if prepared_graph.brain != planned_graph.brain
        || prepared_graph.owner != planned_graph.owner
        || prepared_graph.producer != planned_graph.producer
        || prepared_graph.edge_count != planned_graph.edge_count
        || prepared_graph.logical_edges != planned_graph.logical_edges
        || prepared_graph.node_count != planned_graph.node_count
        || prepared_graph.logical_nodes != planned_graph.logical_nodes
        || prepared_graph.core != planned_graph.core
    {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "prepared graph summary differs from desired plan",
        ));
    }
    Ok(())
}

fn tuple_target<'a>(plan: &'a DesiredPublicationPlanV1, tuple: &ParsedTuple) -> Result<&'a str> {
    let target = match tuple.projection {
        ProjectionKind::Data => plan
            .identity
            .data
            .iter()
            .find(|row| {
                tuple.resource.as_protocol_str()
                    == format!("data/{}", row.physical.as_protocol_str())
            })
            .map(|row| row.physical.as_protocol_str()),
        ProjectionKind::Catalog => Some(plan.identity.catalog.storage.as_ref()),
        ProjectionKind::GraphEdge => Some(plan.identity.graph.edges_index.as_ref()),
        ProjectionKind::GraphNode => Some(plan.identity.graph.nodes_index.as_ref()),
    };
    target.ok_or_else(|| {
        error(
            ProtocolErrorKind::CrossFieldMismatch,
            "desired-plan replay tuple has no matching projection target",
        )
    })
}

fn validate_replay_evidence(
    prepared: &PreparedInputSummaryV1,
    plan: &DesiredPublicationPlanV1,
    tuple: &ParsedTuple,
    evidence: &ReplayEvidenceV1,
) -> Result<()> {
    match (tuple.projection, evidence) {
        (
            ProjectionKind::Data,
            ReplayEvidenceV1::Data {
                ids,
                content,
                prepared_payload,
            },
        ) => {
            let planned = plan
                .identity
                .data
                .iter()
                .find(|row| {
                    tuple.resource.as_protocol_str()
                        == format!("data/{}", row.physical.as_protocol_str())
                })
                .ok_or_else(|| {
                    error(
                        ProtocolErrorKind::CrossFieldMismatch,
                        "data replay evidence has no desired-plan row",
                    )
                })?;
            let prepared = prepared
                .data
                .iter()
                .find(|row| row.slug == planned.slug)
                .ok_or_else(|| {
                    error(
                        ProtocolErrorKind::CrossFieldMismatch,
                        "data replay evidence has no prepared-input row",
                    )
                })?;
            if ids != &planned.ids
                || ids != &prepared.ids
                || content != &planned.content
                || content != &prepared.content
                || prepared_payload != &prepared.payload
            {
                return Err(error(
                    ProtocolErrorKind::CrossFieldMismatch,
                    "data replay content differs from prepared input or desired plan",
                ));
            }
        }
        (
            ProjectionKind::Catalog,
            ReplayEvidenceV1::Catalog {
                ids,
                content,
                prepared_payload,
            },
        ) => {
            if ids != &plan.identity.catalog.ids
                || ids != &prepared.catalog.ids
                || content != &plan.identity.catalog.content
                || content != &prepared.catalog.content
                || prepared_payload != &prepared.catalog.payload
            {
                return Err(error(
                    ProtocolErrorKind::CrossFieldMismatch,
                    "catalog replay content differs from prepared input or desired plan",
                ));
            }
        }
        (
            ProjectionKind::GraphEdge,
            ReplayEvidenceV1::GraphEdge {
                logical,
                physical_ids,
            },
        ) => {
            if logical != &plan.identity.graph.logical_edges
                || logical != &prepared.graph.logical_edges
                || physical_ids != &plan.identity.graph.edge_ids
            {
                return Err(error(
                    ProtocolErrorKind::CrossFieldMismatch,
                    "graph-edge replay content differs from prepared input or desired plan",
                ));
            }
        }
        (
            ProjectionKind::GraphNode,
            ReplayEvidenceV1::GraphNode {
                logical,
                physical_ids,
            },
        ) => {
            if logical != &plan.identity.graph.logical_nodes
                || logical != &prepared.graph.logical_nodes
                || physical_ids != &plan.identity.graph.node_ids
            {
                return Err(error(
                    ProtocolErrorKind::CrossFieldMismatch,
                    "graph-node replay content differs from prepared input or desired plan",
                ));
            }
        }
        _ => {
            return Err(error(
                ProtocolErrorKind::CrossFieldMismatch,
                "replay evidence kind differs from desired-plan tuple",
            ));
        }
    }
    Ok(())
}

fn validate_expected(
    expected: &ExpectedPublicationV1,
    plan: &DesiredPublicationPlanV1,
) -> Result<()> {
    let i = &plan.identity;
    if expected.owner() != &i.owner || expected.sequence() != i.expected {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "expected publication owner/sequence mismatch",
        ));
    }
    if let Some(prefix) = expected.prefix() {
        if prefix != &i.prefix
            || expected.root() != Some(&i.root)
            || expected.incarnation() != Some(&i.incarnation)
        {
            return Err(error(
                ProtocolErrorKind::CrossFieldMismatch,
                "present predecessor identity mismatch",
            ));
        }
    } else if i.expected.get() != 0 {
        return Err(error(
            ProtocolErrorKind::CrossFieldMismatch,
            "absent predecessor requires expected sequence zero",
        ));
    }
    Ok(())
}
fn compute_digest(
    expected: &ExpectedPublicationV1,
    plan: &DesiredPublicationPlanV1,
) -> SyncBeginDigest {
    let mut body = Encoder::default();
    body.u32(1);
    body.raw(&expected.binary_body());
    body.string(expected.digest().as_rendered_str());
    body.u64(plan.canonical_preimage().canonical_preimage().len() as u64);
    body.raw(plan.canonical_preimage().canonical_preimage());
    body.string(plan.digest().as_rendered_str());
    body.string(plan.prepared_input_digest().as_rendered_str());
    body.string(plan.replay_set_digest().as_rendered_str());
    let mut p = Encoder::domain(b"xerj-sync-begin-v1\0");
    p.raw(&body.finish());
    SyncBeginDigest::from_preimage(&p.finish())
}
fn render_json(
    expected: &ExpectedPublicationV1,
    plan: &DesiredPublicationPlanV1,
    digest: &SyncBeginDigest,
) -> Vec<u8> {
    let encoded = STANDARD.encode(plan.canonical_preimage().canonical_preimage());
    let mut out = b"{\"format_version\":1,\"expected_publication\":".to_vec();
    out.extend_from_slice(expected.canonical_json().canonical_json());
    out.extend_from_slice(format!(",\"expected_publication_digest\":{},\"canonical_plan_bytes\":{},\"plan_digest\":{},\"prepared_input_digest\":{},\"replay_set_digest\":{},\"sync_begin_digest\":{}}}",canonical_json::json_string(expected.digest().as_rendered_str()),canonical_json::json_string(&encoded),canonical_json::json_string(plan.digest().as_rendered_str()),canonical_json::json_string(plan.prepared_input_digest().as_rendered_str()),canonical_json::json_string(plan.replay_set_digest().as_rendered_str()),canonical_json::json_string(digest.as_rendered_str())).as_bytes());
    out
}
