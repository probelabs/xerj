use std::str::FromStr;

use xerj_corpus_publication::*;

#[allow(dead_code)]
#[path = "support/reference_codec.rs"]
mod reference_codec;

fn row<T>(result: Result<T, ProtocolError>) -> T {
    result.unwrap()
}

fn artifact_for(artifacts: &[ReplayArtifactV1], projection: ProjectionKind) -> &ReplayArtifactV1 {
    let mut matches = artifacts
        .iter()
        .filter(|artifact| artifact.projection_kind() == projection);
    let artifact = matches.next().expect("projection replay artifact");
    assert!(matches.next().is_none(), "projection must be unique");
    artifact
}

fn data_artifact_containing<'a>(
    artifacts: &'a [ReplayArtifactV1],
    needle: &str,
) -> &'a ReplayArtifactV1 {
    let mut matches = artifacts.iter().filter(|artifact| {
        artifact.projection_kind() == ProjectionKind::Data
            && std::str::from_utf8(artifact.bytes().artifact_bytes())
                .unwrap()
                .contains(needle)
    });
    let artifact = matches.next().expect("matching data replay artifact");
    assert!(matches.next().is_none(), "data artifact must be unique");
    artifact
}

fn replay_sort_key(artifact: &ReplayArtifactV1) -> (String, &str, String, &str) {
    (
        artifact.projection_kind().to_string(),
        artifact.resource_key().as_protocol_str(),
        artifact.kind().to_string(),
        artifact.digest().as_rendered_str(),
    )
}

fn ordering_fixture(reverse: bool) -> (PlannedCorpusV1, Vec<String>) {
    let manifest = ManifestV1::parse_json(
        br#"{"entries":[{"id":"doc-a","path":"a.md"},{"id":"doc-b","path":"b.md"},{"id":"doc-z","path":"z.md"}],"format_version":1,"root_identity":"/ordering"}"#,
    )
    .unwrap();

    let mut alpha_documents = vec![
        row(DataDocumentV1::parse_source(
            DocumentId::from_str("doc-a").unwrap(),
            br#"{"rank":1,"path":"a.md"}"#,
        )),
        row(DataDocumentV1::parse_source(
            DocumentId::from_str("doc-b").unwrap(),
            br#"{"path":"b.md","rank":2}"#,
        )),
    ];
    if reverse {
        alpha_documents.reverse();
    }
    let alpha_mapping = if reverse {
        br#"{"z":{"type":"keyword"},"a":{"type":"text"}}"#.as_slice()
    } else {
        br#"{"a":{"type":"text"},"z":{"type":"keyword"}}"#.as_slice()
    };
    let alpha = DataRouteInputV1::new(
        DataSlug::from_str("alpha").unwrap(),
        LogicalIndexName::from_str("life-alpha").unwrap(),
        DataMappingV1::parse_json(alpha_mapping).unwrap(),
        alpha_documents,
    )
    .unwrap();
    let zeta = DataRouteInputV1::new(
        DataSlug::from_str("zeta").unwrap(),
        LogicalIndexName::from_str("life-zeta").unwrap(),
        DataMappingV1::parse_json(br#"{"enabled":true}"#).unwrap(),
        vec![row(DataDocumentV1::parse_source(
            DocumentId::from_str("doc-z").unwrap(),
            br#"{"path":"z.md","rank":26}"#,
        ))],
    )
    .unwrap();
    let data = if reverse {
        vec![zeta, alpha]
    } else {
        vec![alpha, zeta]
    };

    let mut wrappers = vec![
        row(CatalogWrapperV1::parse_public_source(
            WrapperId::from_str("wrap-a").unwrap(),
            br#"{"canonical":"a"}"#,
        )),
        row(CatalogWrapperV1::parse_public_source(
            WrapperId::from_str("wrap-z").unwrap(),
            br#"{"canonical":"z"}"#,
        )),
    ];
    if reverse {
        wrappers.reverse();
    }
    let catalog = CatalogInputV1::new(
        CatalogMappingV1::parse_json(br#"{"enabled":false}"#).unwrap(),
        wrappers,
    )
    .unwrap();

    let edge_a = LogicalEdgeRowV1::parse_json(br#"{"src":"doc-a","dst":"doc-b","type":"alpha","weight":1,"confidence":0.5,"valid_at":0,"created_at":0,"detector":"ordering@1","schema_version":1,"src_file":"a.md","evidence":{"quote":"a to b","source":"a.md","offset":0}}"#).unwrap();
    let edge_z = LogicalEdgeRowV1::parse_json(br#"{"src":"doc-z","dst":"doc-a","type":"zeta","weight":2,"confidence":1,"valid_at":1,"created_at":1,"detector":"ordering@1","schema_version":1,"src_file":"z.md","evidence":{"quote":"z to a","source":"z.md","offset":1}}"#).unwrap();
    let mut edge_ids = vec![
        edge_a.logical_id().as_lower_hex().to_owned(),
        edge_z.logical_id().as_lower_hex().to_owned(),
    ];
    edge_ids.sort();
    let edges = if reverse {
        vec![edge_z, edge_a]
    } else {
        vec![edge_a, edge_z]
    };

    let mut nodes = vec![
        row(LogicalNodeRowV1::parse_json(
            br#"{"source_index":"life-alpha","logical_node_id":"doc-a","title":"A","preview":null,"path":"a.md"}"#,
        )),
        row(LogicalNodeRowV1::parse_json(
            br#"{"source_index":"life-alpha","logical_node_id":"doc-b","title":"B","preview":null,"path":"b.md"}"#,
        )),
        row(LogicalNodeRowV1::parse_json(
            br#"{"source_index":"life-zeta","logical_node_id":"doc-z","title":"Z","preview":null,"path":"z.md"}"#,
        )),
    ];
    if reverse {
        nodes.reverse();
    }
    let graph = GraphInputV1::new(
        BrainName::from_str("life").unwrap(),
        ExtractorIdentity::from_str("ordering@1").unwrap(),
        ExtractorConfigV1::parse_json(br#"{"z":0,"a":1}"#).unwrap(),
        GraphEdgeMappingV1::parse_json(br#"{"z":{},"a":{}}"#).unwrap(),
        GraphNodeMappingV1::parse_json(br#"{"z":{},"a":{}}"#).unwrap(),
        edges,
        nodes,
    )
    .unwrap();

    let input = PrepareCorpusInputV1::new(
        RootIdentity::from_str("/ordering").unwrap(),
        CorpusPrefix::from_str("life").unwrap(),
        CorpusIncarnationSeed::from_array(std::array::from_fn(|i| 255 - i as u8)),
        manifest,
        data,
        catalog,
        graph,
    )
    .unwrap();
    let prepared = PreparedCorpusV1::prepare(input).unwrap();
    let planned = PlannedCorpusV1::plan(
        prepared,
        SequenceTransitionV1::new(Sequence::new(8), Sequence::new(9)).unwrap(),
        Generation::new(42),
    )
    .unwrap();
    (planned, edge_ids)
}

#[test]
fn every_normative_collection_has_a_two_distinct_value_order_oracle() {
    let (forward, expected_edge_ids) = ordering_fixture(false);
    let (reverse, reverse_edge_ids) = ordering_fixture(true);
    let fixture: serde_json::Value =
        serde_json::from_slice(include_bytes!("../testdata/review11-v1/goldens.json")).unwrap();
    let ordering = &fixture["ordering_matrix"];
    let planned_oracle = &ordering["planned"];
    let prepared_oracle = &ordering["prepared"];
    let reversed_manifest = ManifestV1::parse_json(
        br#"{"entries":[{"id":"doc-z","path":"z.md"},{"id":"doc-b","path":"b.md"},{"id":"doc-a","path":"a.md"}],"format_version":1,"root_identity":"/ordering"}"#,
    )
    .unwrap();
    let canonical_manifest = ManifestV1::parse_json(
        br#"{"entries":[{"id":"doc-a","path":"a.md"},{"id":"doc-b","path":"b.md"},{"id":"doc-z","path":"z.md"}],"format_version":1,"root_identity":"/ordering"}"#,
    )
    .unwrap();
    assert_ne!(reversed_manifest.digest(), canonical_manifest.digest());

    assert_eq!(reverse_edge_ids, expected_edge_ids);
    assert_eq!(
        expected_edge_ids,
        ordering["canonical_order"]["logical_edge_ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        forward.prepared_input().canonical_preimage(),
        reverse.prepared_input().canonical_preimage()
    );
    assert_eq!(
        forward
            .prepared_input()
            .canonical_preimage()
            .canonical_preimage(),
        reference_codec::decode_padded_base64(
            prepared_oracle["prepared_input"]["preimage_base64"]
                .as_str()
                .unwrap()
        )
    );
    assert_eq!(
        forward.desired_plan().canonical_preimage(),
        reverse.desired_plan().canonical_preimage()
    );
    assert_eq!(
        forward.desired_plan().digest(),
        reverse.desired_plan().digest()
    );
    assert_eq!(
        forward.desired_plan().digest().as_rendered_str(),
        planned_oracle["desired_plan"]["rendered"].as_str().unwrap()
    );
    assert_eq!(
        forward
            .desired_plan()
            .canonical_preimage()
            .canonical_preimage(),
        reference_codec::decode_padded_base64(
            planned_oracle["desired_plan"]["preimage_base64"]
                .as_str()
                .unwrap()
        )
    );
    assert_eq!(
        (
            forward
                .prepared_input()
                .canonical_preimage()
                .canonical_preimage()
                .len(),
            forward.prepared_input().digest().as_rendered_str(),
            forward
                .desired_plan()
                .canonical_preimage()
                .canonical_preimage()
                .len(),
        ),
        (
            prepared_oracle["prepared_input"]["preimage_length"]
                .as_u64()
                .unwrap() as usize,
            prepared_oracle["prepared_input"]["rendered"]
                .as_str()
                .unwrap(),
            planned_oracle["desired_plan"]["preimage_length"]
                .as_u64()
                .unwrap() as usize,
        )
    );

    let artifacts = forward.replay_artifacts();
    let artifact_oracles = planned_oracle["artifacts"].as_array().unwrap();
    assert_eq!(artifacts.len(), artifact_oracles.len());
    for (actual, oracle) in artifacts.iter().zip(artifact_oracles) {
        assert_eq!(actual.kind().to_string(), oracle["kind"].as_str().unwrap());
        assert_eq!(
            actual.projection_kind().to_string(),
            oracle["projection_kind"].as_str().unwrap()
        );
        assert_eq!(
            actual.resource_key().as_protocol_str(),
            oracle["resource_key"].as_str().unwrap()
        );
        assert_eq!(
            actual.byte_length(),
            oracle["byte_length"].as_u64().unwrap()
        );
        assert_eq!(
            actual.operation_count(),
            oracle["operation_count"].as_u64().unwrap()
        );
        assert_eq!(
            actual.digest().as_rendered_str(),
            oracle["digest"]["rendered"].as_str().unwrap()
        );
        assert_eq!(
            actual.bytes().artifact_bytes(),
            reference_codec::decode_padded_base64(oracle["bytes_base64"].as_str().unwrap())
        );
    }
    assert_eq!(
        artifacts
            .iter()
            .map(ReplayArtifactV1::kind)
            .collect::<Vec<_>>(),
        vec![
            ReplayArtifactKind::CatalogBulkNdjson,
            ReplayArtifactKind::DataBulkNdjson,
            ReplayArtifactKind::DataBulkNdjson,
            ReplayArtifactKind::GraphEdgeBulkNdjson,
            ReplayArtifactKind::GraphNodeBulkNdjson,
        ]
    );
    assert!(
        artifacts
            .windows(2)
            .all(|pair| replay_sort_key(&pair[0]) < replay_sort_key(&pair[1])),
        "fresh bundle artifacts must use the desired plan's canonical tuple order"
    );
    assert_eq!(artifacts.len(), reverse.replay_artifacts().len());
    for (left, right) in artifacts.iter().zip(reverse.replay_artifacts()) {
        assert_eq!(left.kind(), right.kind());
        assert_eq!(left.resource_key(), right.resource_key());
        assert_eq!(left.digest(), right.digest());
        assert_eq!(left.bytes(), right.bytes());
    }

    let alpha_artifact = data_artifact_containing(artifacts, r#""_id":"doc-a""#);
    let alpha = std::str::from_utf8(alpha_artifact.bytes().artifact_bytes()).unwrap();
    assert!(alpha.find(r#""_id":"doc-a""#).unwrap() < alpha.find(r#""_id":"doc-b""#).unwrap());
    let catalog_artifact = artifact_for(artifacts, ProjectionKind::Catalog);
    let catalog = std::str::from_utf8(catalog_artifact.bytes().artifact_bytes()).unwrap();
    assert!(
        catalog.find(r#""_id":"wrap-a""#).unwrap() < catalog.find(r#""_id":"wrap-z""#).unwrap()
    );
    let edge_artifact = artifact_for(artifacts, ProjectionKind::GraphEdge);
    let edges = std::str::from_utf8(edge_artifact.bytes().artifact_bytes()).unwrap();
    assert!(
        edges.find(&expected_edge_ids[0]).unwrap() < edges.find(&expected_edge_ids[1]).unwrap()
    );
    let node_artifact = artifact_for(artifacts, ProjectionKind::GraphNode);
    let nodes = std::str::from_utf8(node_artifact.bytes().artifact_bytes()).unwrap();
    let node_a = nodes.find(r#""logical_node_id":"doc-a""#).unwrap();
    let node_b = nodes.find(r#""logical_node_id":"doc-b""#).unwrap();
    let node_z = nodes.find(r#""logical_node_id":"doc-z""#).unwrap();
    assert!(node_a < node_b && node_b < node_z);

    let resources = forward.desired_plan().reserved_resource_keys();
    assert!(resources.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(resources.len(), artifacts.len());
    for artifact in artifacts {
        assert!(resources.binary_search(artifact.resource_key()).is_ok());
    }

    let mappings = forward.desired_plan().mapping_reservations();
    assert_eq!(mappings.len(), artifacts.len());
    assert!(mappings.windows(2).all(|pair| {
        (
            pair[0].projection_kind().to_string(),
            pair[0].resource_key().as_protocol_str(),
        ) < (
            pair[1].projection_kind().to_string(),
            pair[1].resource_key().as_protocol_str(),
        )
    }));
    for (mapping, artifact) in mappings.iter().zip(artifacts) {
        assert_eq!(mapping.projection_kind(), artifact.projection_kind());
        assert_eq!(mapping.resource_key(), artifact.resource_key());
    }

    assert_eq!(
        resources
            .iter()
            .map(ResourceKey::as_protocol_str)
            .collect::<Vec<_>>(),
        ordering["canonical_order"]["reserved_resource_keys"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect::<Vec<_>>()
    );

    let publication_oracle = &ordering["prior_publication"]["publication"];
    let publication_json = publication_oracle["canonical_json"].as_str().unwrap();
    let publication = CorpusPublicationV1::parse_closed_json(publication_json.as_bytes()).unwrap();
    assert_eq!(
        publication.digest().as_rendered_str(),
        publication_oracle["rendered"].as_str().unwrap()
    );
    assert_eq!(
        publication.canonical_json().canonical_json(),
        publication_json.as_bytes()
    );
    assert_eq!(
        publication_oracle["preimage_length"].as_u64().unwrap() as usize,
        reference_codec::decode_padded_base64(
            publication_oracle["preimage_base64"].as_str().unwrap()
        )
        .len()
    );
}
