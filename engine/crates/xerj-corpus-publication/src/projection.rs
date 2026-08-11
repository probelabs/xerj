use crate::{
    canonical_json::{self, JsonValue},
    codec::Encoder,
    digest::{
        CatalogGenerationIncarnationId, CatalogProjectionDigest, DataProjectionDigest,
        EdgePhysicalIdSetDigest, GenerationId, GraphProjectionDigest, GraphToken,
        NodePhysicalIdSetDigest, ReplaySetDigest, TransactionId,
    },
    error::{error, ProtocolErrorKind, Result},
    identity,
    plan::{self, MappingReservationV1},
    prepared::PreparedCorpusV1,
    replay::{ProjectionKind, ReplayArtifactKind, ReplayArtifactV1},
    scalar::{Generation, PhysicalDataName, ResourceKey},
};

pub(crate) const CATALOG_INDEX: &str = ".xerj-autoindex-catalog-generations-v1";
pub(crate) const NODE_INDEX: &str = ".xerj-autoindex-graph-nodes-v1";

pub(crate) struct DerivedDataEntry {
    pub(crate) prepared_index: usize,
    pub(crate) physical: PhysicalDataName,
    pub(crate) artifact_index: usize,
}
pub(crate) struct DerivedData {
    pub(crate) projection: DataProjectionDigest,
    pub(crate) entries: Vec<DerivedDataEntry>,
}
pub(crate) struct DerivedCatalog {
    pub(crate) generation_id: GenerationId,
    pub(crate) incarnation: CatalogGenerationIncarnationId,
    pub(crate) projection: CatalogProjectionDigest,
    pub(crate) artifact_index: usize,
}
pub(crate) struct DerivedGraph {
    pub(crate) token: GraphToken,
    pub(crate) edge_id_set: EdgePhysicalIdSetDigest,
    pub(crate) node_id_set: NodePhysicalIdSetDigest,
    pub(crate) projection: GraphProjectionDigest,
    pub(crate) edge_artifact_index: usize,
    pub(crate) node_artifact_index: usize,
    pub(crate) edges_index: Box<str>,
}
pub(crate) struct DerivedPlan {
    pub(crate) tx: TransactionId,
    pub(crate) data: DerivedData,
    pub(crate) catalog: DerivedCatalog,
    pub(crate) graph: DerivedGraph,
    pub(crate) artifacts: Vec<ReplayArtifactV1>,
    pub(crate) replay_set: ReplaySetDigest,
    pub(crate) mapping_reservations: Vec<MappingReservationV1>,
    pub(crate) resource_keys: Vec<ResourceKey>,
    pub(crate) quota_charge: u64,
}

pub(crate) fn derive(
    prepared: &PreparedCorpusV1,
    expected: crate::scalar::Sequence,
    desired: crate::scalar::Sequence,
    generation: Generation,
) -> Result<DerivedPlan> {
    let tx = identity::transaction(
        &prepared.owner,
        &prepared.incarnation,
        expected,
        desired,
        prepared.manifest.digest(),
        prepared.prepared_input.digest(),
    );
    let generation_id =
        identity::generation_id(&prepared.owner, &prepared.incarnation, generation, &tx);
    let mut artifacts = Vec::new();
    let mut data_entries = Vec::new();
    for (index, row) in prepared.data.iter().enumerate() {
        let physical = identity::physical_data_name(
            &prepared.owner,
            &prepared.incarnation,
            &tx,
            prepared.manifest.digest(),
            generation,
            &row.input.slug,
        )?;
        let resource = ResourceKey::from_generated(format!("data/{}", physical.as_protocol_str()))?;
        let mut bytes = Vec::new();
        for doc in &row.input.documents {
            bytes.extend_from_slice(
                format!(
                    "{{\"index\":{{\"_id\":{},\"_index\":{}}}}}\n",
                    canonical_json::json_string(doc.id.as_protocol_str()),
                    canonical_json::json_string(physical.as_protocol_str())
                )
                .as_bytes(),
            );
            bytes.extend_from_slice(&doc.source.canonical);
            bytes.push(b'\n');
        }
        let artifact_index = artifacts.len();
        artifacts.push(ReplayArtifactV1::new(
            ReplayArtifactKind::DataBulkNdjson,
            ProjectionKind::Data,
            resource,
            row.input.documents.len() as u64,
            bytes,
        ));
        data_entries.push(DerivedDataEntry {
            prepared_index: index,
            physical,
            artifact_index,
        });
    }
    let mut data_projection = Encoder::domain(b"xerj-data-projection-v1\0");
    data_projection.u64(generation.get());
    data_projection.array_len(data_entries.len());
    for entry in &data_entries {
        let row = &prepared.data[entry.prepared_index];
        data_projection.string(row.input.slug.as_protocol_str());
        data_projection.string(row.input.logical_index.as_protocol_str());
        data_projection.string(entry.physical.as_protocol_str());
        data_projection.string(row.input.mapping.digest.as_rendered_str());
        data_projection.u64(row.input.documents.len() as u64);
        data_projection.string(row.id_digest.as_rendered_str());
        data_projection.string(row.content_digest.as_rendered_str());
    }
    let data = DerivedData {
        projection: DataProjectionDigest::from_preimage(&data_projection.finish()),
        entries: data_entries,
    };

    let mut catalog_projection = Encoder::domain(b"xerj-catalog-projection-v1\0");
    catalog_projection.string(prepared.owner.as_rendered_str());
    catalog_projection.string(prepared.incarnation.as_rendered_str());
    catalog_projection.u64(generation.get());
    catalog_projection.string(generation_id.as_rendered_str());
    catalog_projection.u64(prepared.catalog.input.wrappers.len() as u64);
    catalog_projection.string(prepared.catalog.wrapper_digest.as_rendered_str());
    let catalog_projection = CatalogProjectionDigest::from_preimage(&catalog_projection.finish());
    let catalog_incarnation = identity::catalog_incarnation(
        &prepared.owner,
        &prepared.incarnation,
        generation,
        &tx,
        &catalog_projection,
    );
    let catalog_resource = ResourceKey::from_generated(format!(
        "catalog/{CATALOG_INDEX}/{}",
        generation_id.as_rendered_str()
    ))?;
    let mut catalog_bytes = Vec::new();
    for row in &prepared.catalog.input.wrappers {
        catalog_bytes.extend_from_slice(
            format!(
                "{{\"index\":{{\"_id\":{},\"_index\":{},\"generation\":{}}}}}\n",
                canonical_json::json_string(row.id.as_protocol_str()),
                canonical_json::json_string(CATALOG_INDEX),
                canonical_json::json_string(generation_id.as_rendered_str())
            )
            .as_bytes(),
        );
        catalog_bytes.extend_from_slice(&row.source.canonical);
        catalog_bytes.push(b'\n');
    }
    let catalog_artifact_index = artifacts.len();
    artifacts.push(ReplayArtifactV1::new(
        ReplayArtifactKind::CatalogBulkNdjson,
        ProjectionKind::Catalog,
        catalog_resource,
        prepared.catalog.input.wrappers.len() as u64,
        catalog_bytes,
    ));
    let catalog = DerivedCatalog {
        generation_id,
        incarnation: catalog_incarnation,
        projection: catalog_projection,
        artifact_index: catalog_artifact_index,
    };

    let token = identity::graph_token(
        &prepared.owner,
        &prepared.incarnation,
        generation,
        &tx,
        &prepared.graph.core_digest,
    );
    let edge_ids: Vec<_> = prepared
        .graph
        .input
        .edges
        .iter()
        .map(|row| {
            identity::edge_physical_id(
                &prepared.owner,
                &prepared.incarnation,
                generation,
                &token,
                row.logical_id.as_lower_hex(),
            )
        })
        .collect();
    let node_ids: Vec<_> = prepared
        .graph
        .input
        .nodes
        .iter()
        .map(|row| {
            identity::node_physical_id(
                &prepared.owner,
                &prepared.incarnation,
                generation,
                &token,
                &row.source_index,
                &row.logical_node_id,
            )
        })
        .collect();
    let mut sorted_edges: Vec<_> = edge_ids.iter().map(|v| v.as_rendered_str()).collect();
    sorted_edges.sort_unstable();
    let mut edge_set = Encoder::domain(b"xerj-graph-edge-physical-ids-v1\0");
    edge_set.array_len(sorted_edges.len());
    for id in sorted_edges {
        edge_set.string(id);
    }
    let edge_id_set = EdgePhysicalIdSetDigest::from_preimage(&edge_set.finish());
    let mut sorted_nodes: Vec<_> = node_ids.iter().map(|v| v.as_rendered_str()).collect();
    sorted_nodes.sort_unstable();
    let mut node_set = Encoder::domain(b"xerj-graph-node-physical-ids-v1\0");
    node_set.array_len(sorted_nodes.len());
    for id in sorted_nodes {
        node_set.string(id);
    }
    let node_id_set = NodePhysicalIdSetDigest::from_preimage(&node_set.finish());
    let mut graph_projection = Encoder::domain(b"xerj-graph-projection-v1\0");
    crate::prepared::encode_graph_core_body(
        &mut graph_projection,
        prepared.graph.input.brain.as_protocol_str(),
        &prepared.owner,
        &prepared.graph.producer,
        prepared.graph.input.edges.len() as u64,
        &prepared.graph.logical_edge_digest,
        prepared.graph.input.nodes.len() as u64,
        &prepared.graph.logical_node_digest,
    );
    graph_projection.string(prepared.graph.core_digest.as_rendered_str());
    graph_projection.u64(generation.get());
    graph_projection.string(token.as_rendered_str());
    graph_projection.string(edge_id_set.as_rendered_str());
    graph_projection.string(node_id_set.as_rendered_str());
    let graph_projection = GraphProjectionDigest::from_preimage(&graph_projection.finish());
    let edges_index: Box<str> = format!(
        ".xerj-memory-{}-edges",
        prepared.graph.input.brain.as_protocol_str()
    )
    .into_boxed_str();
    let edge_resource = ResourceKey::from_generated(format!(
        "graph-edge/{edges_index}/{}",
        token.as_rendered_str()
    ))?;
    let node_resource = ResourceKey::from_generated(format!(
        "graph-node/{NODE_INDEX}/{}",
        token.as_rendered_str()
    ))?;
    let mut edge_bytes = Vec::new();
    for (row, physical) in prepared.graph.input.edges.iter().zip(&edge_ids) {
        edge_bytes.extend_from_slice(
            format!(
                "{{\"index\":{{\"_id\":{},\"_index\":{}}}}}\n",
                canonical_json::json_string(physical.as_rendered_str()),
                canonical_json::json_string(&edges_index)
            )
            .as_bytes(),
        );
        let source = add_members(
            &row.value,
            vec![
                (
                    "corpus_incarnation",
                    JsonValue::String(prepared.incarnation.as_rendered_str().to_owned()),
                ),
                ("edge_scope", JsonValue::String("generated".into())),
                (
                    "graph_generation",
                    JsonValue::Number(generation.get().into()),
                ),
                (
                    "graph_owner",
                    JsonValue::String(prepared.owner.as_rendered_str().to_owned()),
                ),
                (
                    "graph_producer",
                    JsonValue::String(prepared.graph.producer.as_rendered_str().to_owned()),
                ),
                (
                    "logical_edge_id",
                    JsonValue::String(row.logical_id.as_lower_hex().to_owned()),
                ),
                (
                    "physical_id",
                    JsonValue::String(physical.as_rendered_str().to_owned()),
                ),
                ("tx_id", JsonValue::String(tx.as_rendered_str().to_owned())),
            ],
        )?;
        edge_bytes.extend_from_slice(&canonical_json::canonicalize(&source));
        edge_bytes.push(b'\n');
    }
    let edge_artifact_index = artifacts.len();
    artifacts.push(ReplayArtifactV1::new(
        ReplayArtifactKind::GraphEdgeBulkNdjson,
        ProjectionKind::GraphEdge,
        edge_resource,
        edge_ids.len() as u64,
        edge_bytes,
    ));
    let mut node_bytes = Vec::new();
    for (row, physical) in prepared.graph.input.nodes.iter().zip(&node_ids) {
        node_bytes.extend_from_slice(
            format!(
                "{{\"index\":{{\"_id\":{},\"_index\":{}}}}}\n",
                canonical_json::json_string(physical.as_rendered_str()),
                canonical_json::json_string(NODE_INDEX)
            )
            .as_bytes(),
        );
        let source = add_members(
            &row.value,
            vec![
                (
                    "corpus_incarnation",
                    JsonValue::String(prepared.incarnation.as_rendered_str().to_owned()),
                ),
                ("doc_kind", JsonValue::String("generated".into())),
                (
                    "graph_generation",
                    JsonValue::Number(generation.get().into()),
                ),
                (
                    "graph_owner",
                    JsonValue::String(prepared.owner.as_rendered_str().to_owned()),
                ),
                (
                    "physical_id",
                    JsonValue::String(physical.as_rendered_str().to_owned()),
                ),
                ("tx_id", JsonValue::String(tx.as_rendered_str().to_owned())),
            ],
        )?;
        node_bytes.extend_from_slice(&canonical_json::canonicalize(&source));
        node_bytes.push(b'\n');
    }
    let node_artifact_index = artifacts.len();
    artifacts.push(ReplayArtifactV1::new(
        ReplayArtifactKind::GraphNodeBulkNdjson,
        ProjectionKind::GraphNode,
        node_resource,
        node_ids.len() as u64,
        node_bytes,
    ));
    let graph = DerivedGraph {
        token,
        edge_id_set,
        node_id_set,
        projection: graph_projection,
        edge_artifact_index,
        node_artifact_index,
        edges_index,
    };

    let replay_set = compute_replay_set(&artifacts);
    let mut resource_keys: Vec<_> = artifacts.iter().map(|v| v.resource_key.clone()).collect();
    resource_keys.sort_by(|a, b| a.as_protocol_str().cmp(b.as_protocol_str()));
    if resource_keys.windows(2).any(|w| w[0] == w[1]) {
        return Err(error(
            ProtocolErrorKind::DuplicateTuple,
            "duplicate reserved resource key",
        ));
    }
    let mut mapping_reservations = Vec::with_capacity(artifacts.len());
    for entry in &data.entries {
        let row = &prepared.data[entry.prepared_index];
        let artifact = &artifacts[entry.artifact_index];
        mapping_reservations.push(MappingReservationV1::from_canonical_mapping(
            ProjectionKind::Data,
            artifact.resource_key.clone(),
            row.input.mapping.digest.clone(),
            row.input.mapping.json.canonical.clone(),
        )?);
    }
    let catalog_artifact = &artifacts[catalog.artifact_index];
    mapping_reservations.push(MappingReservationV1::from_canonical_mapping(
        ProjectionKind::Catalog,
        catalog_artifact.resource_key.clone(),
        prepared.catalog.input.mapping.digest.clone(),
        prepared.catalog.input.mapping.json.canonical.clone(),
    )?);
    let edge_artifact = &artifacts[graph.edge_artifact_index];
    mapping_reservations.push(MappingReservationV1::from_canonical_mapping(
        ProjectionKind::GraphEdge,
        edge_artifact.resource_key.clone(),
        prepared.graph.input.edge_mapping.digest.clone(),
        prepared.graph.input.edge_mapping.json.canonical.clone(),
    )?);
    let node_artifact = &artifacts[graph.node_artifact_index];
    mapping_reservations.push(MappingReservationV1::from_canonical_mapping(
        ProjectionKind::GraphNode,
        node_artifact.resource_key.clone(),
        prepared.graph.input.node_mapping.digest.clone(),
        prepared.graph.input.node_mapping.json.canonical.clone(),
    )?);
    mapping_reservations.sort_by(|left, right| {
        (
            left.projection_kind().protocol_str(),
            left.resource_key().as_protocol_str(),
        )
            .cmp(&(
                right.projection_kind().protocol_str(),
                right.resource_key().as_protocol_str(),
            ))
    });
    if mapping_reservations.windows(2).any(|window| {
        window[0].projection_kind() == window[1].projection_kind()
            && window[0].resource_key() == window[1].resource_key()
    }) {
        return Err(error(
            ProtocolErrorKind::DuplicateTuple,
            "duplicate mapping reservation",
        ));
    }
    let quota_charge = plan::compute_quota_charge(
        &mapping_reservations,
        artifacts.iter().map(ReplayArtifactV1::byte_length),
        artifacts.iter().map(ReplayArtifactV1::operation_count),
        resource_keys.len(),
    )?;
    Ok(DerivedPlan {
        tx,
        data,
        catalog,
        graph,
        artifacts,
        replay_set,
        mapping_reservations,
        resource_keys,
        quota_charge,
    })
}

fn add_members(value: &JsonValue, additions: Vec<(&str, JsonValue)>) -> Result<JsonValue> {
    let mut members = canonical_json::object(value, "logical row")?.to_vec();
    for (name, value) in additions {
        if members.iter().any(|(key, _)| key == name) {
            return Err(error(
                ProtocolErrorKind::CrossFieldMismatch,
                "logical row contains generated physical field",
            ));
        }
        members.push((name.into(), value));
    }
    Ok(JsonValue::Object(members))
}

pub(crate) fn sorted_artifact_indices(artifacts: &[ReplayArtifactV1]) -> Vec<usize> {
    let mut indices: Vec<_> = (0..artifacts.len()).collect();
    indices.sort_by(|&a, &b| compare_artifacts(&artifacts[a], &artifacts[b]));
    indices
}

fn compare_artifacts(left: &ReplayArtifactV1, right: &ReplayArtifactV1) -> std::cmp::Ordering {
    (
        left.projection_kind.protocol_str(),
        left.resource_key.as_protocol_str(),
        left.kind.protocol_str(),
        left.digest.as_rendered_str(),
    )
        .cmp(&(
            right.projection_kind.protocol_str(),
            right.resource_key.as_protocol_str(),
            right.kind.protocol_str(),
            right.digest.as_rendered_str(),
        ))
}

pub(crate) fn sort_artifacts_canonically(artifacts: &mut [ReplayArtifactV1]) {
    artifacts.sort_by(compare_artifacts);
}

pub(crate) fn encode_replay_tuple(out: &mut Encoder, artifact: &ReplayArtifactV1) {
    out.string(artifact.kind.protocol_str());
    out.string(artifact.projection_kind.protocol_str());
    out.string(artifact.resource_key.as_protocol_str());
    out.u64(artifact.byte_length());
    out.u64(artifact.operation_count);
    out.string(artifact.digest.as_rendered_str());
}

pub(crate) fn compute_replay_set(artifacts: &[ReplayArtifactV1]) -> ReplaySetDigest {
    let indices = sorted_artifact_indices(artifacts);
    let mut out = Encoder::domain(b"xerj-replay-set-v1\0");
    out.array_len(indices.len());
    for index in indices {
        encode_replay_tuple(&mut out, &artifacts[index]);
    }
    ReplaySetDigest::from_preimage(&out.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        digest::{EdgePhysicalId, NodePhysicalId},
        logical_input::{
            CatalogInputV1, DataDocumentV1, DataMappingV1, DataRouteInputV1, ExtractorConfigV1,
            GraphEdgeMappingV1, GraphInputV1, GraphNodeMappingV1, LogicalEdgeRowV1,
            LogicalNodeRowV1, PrepareCorpusInputV1,
        },
        manifest::ManifestV1,
        prepared::PreparedCorpusV1,
        scalar::{
            BrainName, CorpusIncarnationSeed, CorpusPrefix, DataSlug, DocumentId,
            ExtractorIdentity, LogicalIndexName, RootIdentity, Sequence,
        },
    };
    use std::str::FromStr;

    fn prepared_fixture() -> PreparedCorpusV1 {
        let manifest = ManifestV1::parse_json(
            br#"{"entries":[{"id":"doc-a","path":"a.md"},{"id":"doc-b","path":"b.md"}],"format_version":1,"root_identity":"/private-tests"}"#,
        )
        .unwrap();
        let data = DataRouteInputV1::new(
            DataSlug::from_str("docs").unwrap(),
            LogicalIndexName::from_str("life-docs").unwrap(),
            DataMappingV1::parse_json(br#"{"properties":{"body":{"type":"text"}}}"#).unwrap(),
            vec![
                DataDocumentV1::parse_source(
                    DocumentId::from_str("doc-a").unwrap(),
                    br#"{"body":"Alpha"}"#,
                )
                .unwrap(),
                DataDocumentV1::parse_source(
                    DocumentId::from_str("doc-b").unwrap(),
                    br#"{"body":"Beta"}"#,
                )
                .unwrap(),
            ],
        )
        .unwrap();
        let catalog = CatalogInputV1::new(
            crate::logical_input::CatalogMappingV1::parse_json(br#"{"enabled":false}"#).unwrap(),
            Vec::new(),
        )
        .unwrap();
        let edges = vec![
            LogicalEdgeRowV1::parse_json(br#"{"src":"doc-a","dst":"doc-b","type":"a","weight":1,"confidence":1,"valid_at":0,"created_at":0,"detector":"private@1","schema_version":1,"src_file":"a.md","evidence":{"quote":"a","source":"a.md","offset":0}}"#).unwrap(),
            LogicalEdgeRowV1::parse_json(br#"{"src":"doc-b","dst":"doc-a","type":"b","weight":1,"confidence":1,"valid_at":0,"created_at":0,"detector":"private@1","schema_version":1,"src_file":"b.md","evidence":{"quote":"b","source":"b.md","offset":0}}"#).unwrap(),
        ];
        let nodes = vec![
            LogicalNodeRowV1::parse_json(br#"{"source_index":"life-docs","logical_node_id":"doc-b","title":"Beta","preview":null,"path":"b.md"}"#).unwrap(),
            LogicalNodeRowV1::parse_json(br#"{"source_index":"life-docs","logical_node_id":"doc-a","title":"Alpha","preview":null,"path":"a.md"}"#).unwrap(),
        ];
        let graph = GraphInputV1::new(
            BrainName::from_str("life").unwrap(),
            ExtractorIdentity::from_str("private@1").unwrap(),
            ExtractorConfigV1::parse_json(br#"{}"#).unwrap(),
            GraphEdgeMappingV1::parse_json(br#"{"enabled":true}"#).unwrap(),
            GraphNodeMappingV1::parse_json(br#"{"enabled":true}"#).unwrap(),
            edges,
            nodes,
        )
        .unwrap();
        let input = PrepareCorpusInputV1::new(
            RootIdentity::from_str("/private-tests").unwrap(),
            CorpusPrefix::from_str("life").unwrap(),
            CorpusIncarnationSeed::from_array([7; 32]),
            manifest,
            vec![data],
            catalog,
            graph,
        )
        .unwrap();
        PreparedCorpusV1::prepare(input).unwrap()
    }

    fn assert_overflow(result: Result<u64>, detail: &str) {
        let error = result.unwrap_err();
        assert_eq!(error.kind(), ProtocolErrorKind::ArithmeticOverflow);
        assert_eq!(error.to_string(), detail);
    }

    #[test]
    fn every_quota_arithmetic_overflow_path_is_executable() {
        assert_overflow(
            plan::compute_quota_charge(&[], [u64::MAX, 1], [0], 0),
            "artifact charge addition overflow",
        );
        assert_overflow(
            plan::compute_quota_charge(&[], [0], [u64::MAX, 1], 0),
            "operation count addition overflow",
        );
        assert_overflow(
            plan::compute_quota_charge(&[], [0], [u64::MAX / 64 + 1], 0),
            "operation charge multiplication overflow",
        );
        assert_overflow(
            plan::compute_quota_charge(
                &[],
                [0],
                [0],
                usize::try_from(u64::MAX / 4096 + 1).unwrap(),
            ),
            "resource charge multiplication overflow",
        );
        assert_overflow(
            plan::compute_quota_charge(&[], [u64::MAX], [0], 1),
            "stage charge addition overflow",
        );
    }

    #[test]
    fn quota_multiplication_boundaries_accept_the_exact_max_safe_operand() {
        assert_eq!(
            plan::compute_quota_charge(&[], [0], [u64::MAX / 64], 0).unwrap(),
            64 * (u64::MAX / 64)
        );
        assert_eq!(
            plan::compute_quota_charge(&[], [0], [0], usize::try_from(u64::MAX / 4096).unwrap(),)
                .unwrap(),
            4096 * (u64::MAX / 4096)
        );
    }

    #[test]
    fn transaction_identity_excludes_generation_and_derived_replay_inputs() {
        let prepared = prepared_fixture();
        let generation_one = derive(
            &prepared,
            Sequence::new(0),
            Sequence::new(1),
            Generation::new(1),
        )
        .unwrap();
        let generation_seven = derive(
            &prepared,
            Sequence::new(0),
            Sequence::new(1),
            Generation::new(7),
        )
        .unwrap();

        assert_eq!(generation_one.tx, generation_seven.tx);
        assert_ne!(generation_one.replay_set, generation_seven.replay_set);
        assert_ne!(
            generation_one.data.entries[0].physical,
            generation_seven.data.entries[0].physical
        );

        let mut exact = Encoder::domain(b"xerj-autoindex-transaction-v1\0");
        exact.string(prepared.owner.as_rendered_str());
        exact.string(prepared.incarnation.as_rendered_str());
        exact.u64(0);
        exact.u64(1);
        exact.string(prepared.manifest.digest().as_rendered_str());
        exact.string(prepared.prepared_input.digest().as_rendered_str());
        assert_eq!(
            TransactionId::from_preimage(&exact.finish()),
            generation_one.tx
        );
    }

    fn test_resource() -> ResourceKey {
        ResourceKey::from_str(&format!(
            "graph-edge/.xerj-memory-life-edges/xergt1-sha256-{}",
            "0".repeat(64)
        ))
        .unwrap()
    }

    #[test]
    fn replay_tuple_sort_exercises_kind_and_digest_tie_breakers() {
        let resource = test_resource();
        let kind_later = ReplayArtifactV1::new(
            ReplayArtifactKind::DataBulkNdjson,
            ProjectionKind::Data,
            resource.clone(),
            0,
            b"same".to_vec(),
        );
        let kind_earlier = ReplayArtifactV1::new(
            ReplayArtifactKind::CatalogBulkNdjson,
            ProjectionKind::Data,
            resource.clone(),
            0,
            b"same".to_vec(),
        );
        assert_eq!(
            sorted_artifact_indices(&[kind_later, kind_earlier]),
            vec![1, 0]
        );

        let first = ReplayArtifactV1::new(
            ReplayArtifactKind::DataBulkNdjson,
            ProjectionKind::Data,
            resource.clone(),
            0,
            b"first".to_vec(),
        );
        let second = ReplayArtifactV1::new(
            ReplayArtifactKind::DataBulkNdjson,
            ProjectionKind::Data,
            resource,
            0,
            b"second".to_vec(),
        );
        assert_ne!(first.digest, second.digest);
        let expected = if first.digest.as_rendered_str() < second.digest.as_rendered_str() {
            vec![0, 1]
        } else {
            vec![1, 0]
        };
        assert_eq!(sorted_artifact_indices(&[first, second]), expected);
    }

    fn bulk_action_ids(bytes: &[u8]) -> Vec<String> {
        bytes
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .step_by(2)
            .map(|line| {
                let action: serde_json::Value = serde_json::from_slice(line).unwrap();
                action["index"]["_id"].as_str().unwrap().to_owned()
            })
            .collect()
    }

    #[test]
    fn graph_physical_ids_and_set_preimages_are_direct_and_sorted() {
        let prepared = prepared_fixture();
        let generation = Generation::new(7);
        let derived = derive(&prepared, Sequence::new(0), Sequence::new(1), generation).unwrap();

        let edge_ids = prepared
            .graph
            .input
            .edges
            .iter()
            .map(|row| {
                let mut bytes = Encoder::domain(b"xerj-graph-edge-physical-id-v1\0");
                bytes.string(prepared.owner.as_rendered_str());
                bytes.string(prepared.incarnation.as_rendered_str());
                bytes.u64(generation.get());
                bytes.string(derived.graph.token.as_rendered_str());
                bytes.string(row.logical_id.as_lower_hex());
                let direct = EdgePhysicalId::from_preimage(&bytes.finish());
                assert_eq!(
                    direct,
                    identity::edge_physical_id(
                        &prepared.owner,
                        &prepared.incarnation,
                        generation,
                        &derived.graph.token,
                        row.logical_id.as_lower_hex(),
                    )
                );
                direct
            })
            .collect::<Vec<_>>();
        let node_ids = prepared
            .graph
            .input
            .nodes
            .iter()
            .map(|row| {
                let mut bytes = Encoder::domain(b"xerj-graph-node-physical-id-v1\0");
                bytes.string(prepared.owner.as_rendered_str());
                bytes.string(prepared.incarnation.as_rendered_str());
                bytes.u64(generation.get());
                bytes.string(derived.graph.token.as_rendered_str());
                bytes.string(&row.source_index);
                bytes.string(&row.logical_node_id);
                let direct = NodePhysicalId::from_preimage(&bytes.finish());
                assert_eq!(
                    direct,
                    identity::node_physical_id(
                        &prepared.owner,
                        &prepared.incarnation,
                        generation,
                        &derived.graph.token,
                        &row.source_index,
                        &row.logical_node_id,
                    )
                );
                direct
            })
            .collect::<Vec<_>>();

        assert_eq!(
            bulk_action_ids(
                derived.artifacts[derived.graph.edge_artifact_index]
                    .bytes()
                    .artifact_bytes()
            ),
            edge_ids
                .iter()
                .map(|id| id.as_rendered_str().to_owned())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            bulk_action_ids(
                derived.artifacts[derived.graph.node_artifact_index]
                    .bytes()
                    .artifact_bytes()
            ),
            node_ids
                .iter()
                .map(|id| id.as_rendered_str().to_owned())
                .collect::<Vec<_>>()
        );

        let mut sorted_edges = edge_ids
            .iter()
            .map(|id| id.as_rendered_str())
            .collect::<Vec<_>>();
        sorted_edges.sort_unstable();
        let mut edge_set = Encoder::domain(b"xerj-graph-edge-physical-ids-v1\0");
        edge_set.array_len(sorted_edges.len());
        for id in &sorted_edges {
            edge_set.string(id);
        }
        assert_eq!(
            EdgePhysicalIdSetDigest::from_preimage(&edge_set.finish()),
            derived.graph.edge_id_set
        );

        let mut sorted_nodes = node_ids
            .iter()
            .map(|id| id.as_rendered_str())
            .collect::<Vec<_>>();
        sorted_nodes.sort_unstable();
        let mut node_set = Encoder::domain(b"xerj-graph-node-physical-ids-v1\0");
        node_set.array_len(sorted_nodes.len());
        for id in &sorted_nodes {
            node_set.string(id);
        }
        assert_eq!(
            NodePhysicalIdSetDigest::from_preimage(&node_set.finish()),
            derived.graph.node_id_set
        );

        sorted_edges.reverse();
        let mut reversed_edge_set = Encoder::domain(b"xerj-graph-edge-physical-ids-v1\0");
        reversed_edge_set.array_len(sorted_edges.len());
        for id in sorted_edges {
            reversed_edge_set.string(id);
        }
        assert_ne!(
            EdgePhysicalIdSetDigest::from_preimage(&reversed_edge_set.finish()),
            derived.graph.edge_id_set
        );

        sorted_nodes.reverse();
        let mut reversed_node_set = Encoder::domain(b"xerj-graph-node-physical-ids-v1\0");
        reversed_node_set.array_len(sorted_nodes.len());
        for id in sorted_nodes {
            reversed_node_set.string(id);
        }
        assert_ne!(
            NodePhysicalIdSetDigest::from_preimage(&reversed_node_set.finish()),
            derived.graph.node_id_set
        );
    }
}
