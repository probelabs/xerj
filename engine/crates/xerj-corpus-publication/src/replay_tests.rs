use super::*;

fn validate(
    kind: ReplayArtifactKind,
    projection_kind: ProjectionKind,
    resource: &str,
    target: &str,
    bytes: &[u8],
    operation_count: u64,
) -> Result<ReplayArtifactV1> {
    let owner: CorpusOwnerId = rendered("xercpo1-sha256-", 'a').parse().unwrap();
    let incarnation: CorpusIncarnationId = rendered("xercpi1-sha256-", 'b').parse().unwrap();
    let transaction: TransactionId = rendered("xertx1-sha256-", 'c').parse().unwrap();
    let producer: ProducerId = rendered("xerp1-sha256-", 'd').parse().unwrap();
    validate_with_authority(
        kind,
        projection_kind,
        resource,
        target,
        bytes,
        operation_count,
        &owner,
        &incarnation,
        Generation::new(0),
        &transaction,
        &producer,
    )
}

#[allow(clippy::too_many_arguments)]
fn validate_with_authority(
    kind: ReplayArtifactKind,
    projection_kind: ProjectionKind,
    resource: &str,
    target: &str,
    bytes: &[u8],
    operation_count: u64,
    owner: &CorpusOwnerId,
    incarnation: &CorpusIncarnationId,
    generation: Generation,
    transaction: &TransactionId,
    producer: &ProducerId,
) -> Result<ReplayArtifactV1> {
    let resource_key = ResourceKey::from_str(resource).unwrap();
    let digest = crate::prepared::artifact_digest(kind.protocol_str(), bytes);
    validate_replay_artifact_v1(
        ReplayTupleExpectationV1 {
            kind,
            projection_kind,
            resource_key: &resource_key,
            target,
            byte_length: bytes.len() as u64,
            operation_count,
            digest: &digest,
            owner,
            corpus_incarnation: incarnation,
            generation,
            transaction,
            graph_producer: producer,
        },
        bytes.into(),
    )
}

const DATA_TARGET: &str = concat!(
    ".xerj-aidx-d-",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "-g1-s",
    "1111111111111111111111111111111111111111111111111111111111111111",
    "-t",
    "2222222222222222222222222222222222222222222222222222222222222222"
);

#[test]
fn data_and_catalog_pairs_validate_in_normative_order() {
    let data = format!(
        "{{\"index\":{{\"_id\":\"doc-a\",\"_index\":\"{DATA_TARGET}\"}}}}\n{{\"a\":1,\"z\":2}}\n{{\"index\":{{\"_id\":\"doc-z\",\"_index\":\"{DATA_TARGET}\"}}}}\n{{\"body\":\"z\"}}\n"
    );
    let artifact = validate(
        ReplayArtifactKind::DataBulkNdjson,
        ProjectionKind::Data,
        &format!("data/{DATA_TARGET}"),
        DATA_TARGET,
        data.as_bytes(),
        2,
    )
    .unwrap();
    assert_eq!(artifact.operation_count(), 2);
    assert_eq!(artifact.byte_length(), data.len() as u64);

    let generation = format!("xerg1-sha256-{}", "3".repeat(64));
    let catalog = format!(
        "{{\"index\":{{\"_id\":\"wrap-a\",\"_index\":\"{}\",\"generation\":\"{generation}\"}}}}\n{{\"custom\":{{\"nested\":true}}}}\n",
        crate::projection::CATALOG_INDEX,
    );
    validate(
        ReplayArtifactKind::CatalogBulkNdjson,
        ProjectionKind::Catalog,
        &format!(
            "catalog/{}/{}",
            crate::projection::CATALOG_INDEX,
            generation
        ),
        crate::projection::CATALOG_INDEX,
        catalog.as_bytes(),
        1,
    )
    .unwrap();
}

#[test]
fn empty_artifact_requires_a_zero_tuple_but_keeps_positional_identity() {
    validate(
        ReplayArtifactKind::DataBulkNdjson,
        ProjectionKind::Data,
        &format!("data/{DATA_TARGET}"),
        DATA_TARGET,
        b"",
        0,
    )
    .unwrap();

    let resource = ResourceKey::from_str(&format!("data/{DATA_TARGET}")).unwrap();
    let digest = crate::prepared::artifact_digest("data-bulk-ndjson", b"");
    let owner: CorpusOwnerId = rendered("xercpo1-sha256-", 'a').parse().unwrap();
    let incarnation: CorpusIncarnationId = rendered("xercpi1-sha256-", 'b').parse().unwrap();
    let transaction: TransactionId = rendered("xertx1-sha256-", 'c').parse().unwrap();
    let producer: ProducerId = rendered("xerp1-sha256-", 'd').parse().unwrap();
    let err = validate_replay_artifact_v1(
        ReplayTupleExpectationV1 {
            kind: ReplayArtifactKind::DataBulkNdjson,
            projection_kind: ProjectionKind::Data,
            resource_key: &resource,
            target: DATA_TARGET,
            byte_length: 0,
            operation_count: 1,
            digest: &digest,
            owner: &owner,
            corpus_incarnation: &incarnation,
            generation: Generation::new(0),
            transaction: &transaction,
            graph_producer: &producer,
        },
        Box::new([]),
    )
    .unwrap_err();
    assert_eq!(err.kind(), ProtocolErrorKind::CrossFieldMismatch);
}

#[test]
fn ndjson_boundaries_actions_and_canonical_lines_are_strict() {
    let resource = format!("data/{DATA_TARGET}");
    let cases: &[(&[u8], ProtocolErrorKind)] = &[
        (b"{}\r\n{}\r\n", ProtocolErrorKind::NonCanonicalEncoding),
        (b"{}\n\n", ProtocolErrorKind::NonCanonicalEncoding),
        (b"{}\n", ProtocolErrorKind::InvalidJson),
        (b"{}\n{}", ProtocolErrorKind::NonCanonicalEncoding),
        (
            br#"{"index":{"_id":"a","_id":"b","_index":"x"}}
{}
"#,
            ProtocolErrorKind::DuplicateJsonKey,
        ),
        (
            br#"{"index":{"_index":"x","_id":"a"}}
{}
"#,
            ProtocolErrorKind::NonCanonicalEncoding,
        ),
        (
            br#"{"delete":{"_id":"a","_index":"x"}}
{}
"#,
            ProtocolErrorKind::InvalidJson,
        ),
        (
            br#"{"index":{"_id":"a","_index":"x","routing":"r"}}
{}
"#,
            ProtocolErrorKind::InvalidJson,
        ),
        (
            br#"{"index":{"_id":"a","_index":"x"}}
{"a":1,"a":2}
"#,
            ProtocolErrorKind::DuplicateJsonKey,
        ),
        (
            br#"{"index":{"_id":"a","_index":"x"}}
{"z":1,"a":2}
"#,
            ProtocolErrorKind::NonCanonicalEncoding,
        ),
    ];
    for (bytes, kind) in cases {
        let bytes = String::from_utf8(bytes.to_vec())
            .unwrap()
            .replace("\"x\"", &format!("\"{DATA_TARGET}\""));
        let error = validate(
            ReplayArtifactKind::DataBulkNdjson,
            ProjectionKind::Data,
            &resource,
            DATA_TARGET,
            bytes.as_bytes(),
            1,
        )
        .unwrap_err();
        assert_eq!(error.kind(), *kind, "bytes={bytes:?}: {error}");
    }
}

#[test]
fn action_target_tuple_metadata_and_row_order_are_joined() {
    let resource = format!("data/{DATA_TARGET}");
    let wrong_target = b"{\"index\":{\"_id\":\"doc-a\",\"_index\":\"wrong\"}}\n{}\n";
    assert_eq!(
        validate(
            ReplayArtifactKind::DataBulkNdjson,
            ProjectionKind::Data,
            &resource,
            DATA_TARGET,
            wrong_target,
            1,
        )
        .unwrap_err()
        .kind(),
        ProtocolErrorKind::CrossFieldMismatch
    );

    let reversed = format!(
        "{{\"index\":{{\"_id\":\"z\",\"_index\":\"{DATA_TARGET}\"}}}}\n{{}}\n{{\"index\":{{\"_id\":\"a\",\"_index\":\"{DATA_TARGET}\"}}}}\n{{}}\n"
    );
    assert_eq!(
        validate(
            ReplayArtifactKind::DataBulkNdjson,
            ProjectionKind::Data,
            &resource,
            DATA_TARGET,
            reversed.as_bytes(),
            2,
        )
        .unwrap_err()
        .kind(),
        ProtocolErrorKind::NonCanonicalEncoding
    );

    let duplicate = format!(
        "{{\"index\":{{\"_id\":\"a\",\"_index\":\"{DATA_TARGET}\"}}}}\n{{\"n\":1}}\n{{\"index\":{{\"_id\":\"a\",\"_index\":\"{DATA_TARGET}\"}}}}\n{{\"n\":2}}\n"
    );
    assert_eq!(
        validate(
            ReplayArtifactKind::DataBulkNdjson,
            ProjectionKind::Data,
            &resource,
            DATA_TARGET,
            duplicate.as_bytes(),
            2,
        )
        .unwrap_err()
        .kind(),
        ProtocolErrorKind::DuplicateTuple
    );

    let resource_key = ResourceKey::from_str(&resource).unwrap();
    let digest = crate::prepared::artifact_digest("data-bulk-ndjson", b"");
    let owner: CorpusOwnerId = rendered("xercpo1-sha256-", 'a').parse().unwrap();
    let incarnation: CorpusIncarnationId = rendered("xercpi1-sha256-", 'b').parse().unwrap();
    let transaction: TransactionId = rendered("xertx1-sha256-", 'c').parse().unwrap();
    let producer: ProducerId = rendered("xerp1-sha256-", 'd').parse().unwrap();
    let mismatch = validate_replay_artifact_v1(
        ReplayTupleExpectationV1 {
            kind: ReplayArtifactKind::DataBulkNdjson,
            projection_kind: ProjectionKind::Catalog,
            resource_key: &resource_key,
            target: DATA_TARGET,
            byte_length: 0,
            operation_count: 0,
            digest: &digest,
            owner: &owner,
            corpus_incarnation: &incarnation,
            generation: Generation::new(0),
            transaction: &transaction,
            graph_producer: &producer,
        },
        Box::new([]),
    )
    .unwrap_err();
    assert_eq!(mismatch.kind(), ProtocolErrorKind::CrossFieldMismatch);
}

fn rendered(prefix: &str, byte: char) -> String {
    format!("{prefix}{}", byte.to_string().repeat(64))
}

#[test]
fn graph_rows_reconstruct_logical_and_physical_identity() {
    let owner: CorpusOwnerId = rendered("xercpo1-sha256-", '1').parse().unwrap();
    let incarnation: CorpusIncarnationId = rendered("xercpi1-sha256-", '2').parse().unwrap();
    let token: GraphToken = rendered("xergt1-sha256-", '3').parse().unwrap();
    let producer = rendered("xerp1-sha256-", '4');
    let tx = rendered("xertx1-sha256-", '5');
    let edge_target = ".xerj-memory-brain-edges";
    let edge_resource = format!("graph-edge/{edge_target}/{}", token.as_rendered_str());
    let logical_edge = br#"{"confidence":1,"created_at":0,"detector":"test@1","dst":"b","evidence":{"offset":0,"quote":"q","source":"a.md"},"schema_version":1,"src":"a","src_file":"a.md","type":"calls","valid_at":0,"weight":1}"#;
    let logical_edge = LogicalEdgeRowV1::parse_json(logical_edge).unwrap();
    let physical = crate::identity::edge_physical_id(
        &owner,
        &incarnation,
        Generation::new(7),
        &token,
        logical_edge.logical_id.as_lower_hex(),
    );
    let edge_source = format!(
        "{{\"confidence\":1,\"corpus_incarnation\":\"{}\",\"created_at\":0,\"detector\":\"test@1\",\"dst\":\"b\",\"edge_scope\":\"generated\",\"evidence\":{{\"offset\":0,\"quote\":\"q\",\"source\":\"a.md\"}},\"graph_generation\":7,\"graph_owner\":\"{}\",\"graph_producer\":\"{}\",\"logical_edge_id\":\"{}\",\"physical_id\":\"{}\",\"schema_version\":1,\"src\":\"a\",\"src_file\":\"a.md\",\"tx_id\":\"{}\",\"type\":\"calls\",\"valid_at\":0,\"weight\":1}}",
        incarnation,
        owner,
        producer,
        logical_edge.logical_id,
        physical,
        tx,
    );
    let edge = format!(
        "{{\"index\":{{\"_id\":\"{}\",\"_index\":\"{edge_target}\"}}}}\n{edge_source}\n",
        physical,
    );
    let producer_id: ProducerId = producer.parse().unwrap();
    let transaction_id: TransactionId = tx.parse().unwrap();
    validate_with_authority(
        ReplayArtifactKind::GraphEdgeBulkNdjson,
        ProjectionKind::GraphEdge,
        &edge_resource,
        edge_target,
        edge.as_bytes(),
        1,
        &owner,
        &incarnation,
        Generation::new(7),
        &transaction_id,
        &producer_id,
    )
    .unwrap();

    let node_target = crate::projection::NODE_INDEX;
    let node_resource = format!("graph-node/{node_target}/{}", token.as_rendered_str());
    let physical = crate::identity::node_physical_id(
        &owner,
        &incarnation,
        Generation::new(7),
        &token,
        "docs",
        "node-a",
    );
    let node_source = format!(
        "{{\"corpus_incarnation\":\"{}\",\"doc_kind\":\"generated\",\"graph_generation\":7,\"graph_owner\":\"{}\",\"logical_node_id\":\"node-a\",\"path\":\"a.md\",\"physical_id\":\"{}\",\"preview\":null,\"source_index\":\"docs\",\"title\":\"A\",\"tx_id\":\"{}\"}}",
        incarnation, owner, physical, tx,
    );
    let node = format!(
        "{{\"index\":{{\"_id\":\"{}\",\"_index\":\"{node_target}\"}}}}\n{node_source}\n",
        physical,
    );
    validate_with_authority(
        ReplayArtifactKind::GraphNodeBulkNdjson,
        ProjectionKind::GraphNode,
        &node_resource,
        node_target,
        node.as_bytes(),
        1,
        &owner,
        &incarnation,
        Generation::new(7),
        &transaction_id,
        &producer_id,
    )
    .unwrap();

    let wrong_edge = edge.replacen(
        &format!(
            "\"physical_id\":\"{}\"",
            crate::identity::edge_physical_id(
                &owner,
                &incarnation,
                Generation::new(7),
                &token,
                logical_edge.logical_id.as_lower_hex()
            )
        ),
        &format!("\"physical_id\":\"{}\"", rendered("xerge1-sha256-", '6')),
        1,
    );
    assert_eq!(
        validate_with_authority(
            ReplayArtifactKind::GraphEdgeBulkNdjson,
            ProjectionKind::GraphEdge,
            &edge_resource,
            edge_target,
            wrong_edge.as_bytes(),
            1,
            &owner,
            &incarnation,
            Generation::new(7),
            &transaction_id,
            &producer_id,
        )
        .unwrap_err()
        .kind(),
        ProtocolErrorKind::CrossFieldMismatch
    );
}
