#![allow(dead_code)]

use std::str::FromStr;
use xerj_corpus_publication::*;

pub mod reference_codec;

pub fn absent_bundle(generation: u64) -> DurableBeginBundleV1 {
    absent_bundle_with(
        generation,
        br#"{"path":"alpha.md","body":"Alpha links [[beta]]."}"#,
        br#"{"path":"alpha.md","kind":"file"}"#,
        br#"{}"#,
    )
}

pub fn absent_planned(generation: u64) -> PlannedCorpusV1 {
    planned_fixture_with(
        0,
        1,
        generation,
        br#"{"path":"alpha.md","body":"Alpha links [[beta]]."}"#,
        br#"{"path":"alpha.md","kind":"file"}"#,
        br#"{}"#,
    )
}

pub fn absent_bundle_with(
    generation: u64,
    data_source: &[u8],
    catalog_source: &[u8],
    extractor_config: &[u8],
) -> DurableBeginBundleV1 {
    let planned = planned_fixture_with(
        0,
        1,
        generation,
        data_source,
        catalog_source,
        extractor_config,
    );
    let owner = planned.desired_plan().owner().clone();
    DurableBeginBundleV1::build(ExpectedPublicationV1::absent(owner), planned).unwrap()
}

pub fn present_bundle() -> DurableBeginBundleV1 {
    let planned = planned_fixture_with(
        1,
        2,
        2,
        br#"{"path":"alpha.md","body":"Alpha links [[beta]]."}"#,
        br#"{"path":"alpha.md","kind":"file"}"#,
        b"{}",
    );
    let publication = CorpusPublicationV1::parse_closed_json(include_bytes!(
        "../../testdata/review11-v1/publication.json"
    ))
    .unwrap();
    DurableBeginBundleV1::build(
        ExpectedPublicationV1::present(publication).unwrap(),
        planned,
    )
    .unwrap()
}

pub fn absent_bundle_with_data_route_count(route_count: usize) -> DurableBeginBundleV1 {
    assert!(
        matches!(route_count, 0 | 2),
        "test helper supports the zero-route and two-route controls"
    );
    let manifest = ManifestV1::parse_json(
        br#"{"entries":[{"id":"doc-a","path":"alpha.md"},{"id":"doc-b","path":"beta.md"}],"format_version":1,"root_identity":"/r"}"#,
    )
    .unwrap();
    let mut data = Vec::with_capacity(route_count);
    if route_count == 2 {
        data.push(
            DataRouteInputV1::new(
                DataSlug::from_str("docs").unwrap(),
                LogicalIndexName::from_str("life-docs").unwrap(),
                DataMappingV1::parse_json(br#"{"properties":{"body":{"type":"text"}}}"#).unwrap(),
                vec![DataDocumentV1::parse_source(
                    DocumentId::from_str("doc-a").unwrap(),
                    br#"{"body":"Alpha."}"#,
                )
                .unwrap()],
            )
            .unwrap(),
        );
        data.push(
            DataRouteInputV1::new(
                DataSlug::from_str("notes").unwrap(),
                LogicalIndexName::from_str("life-notes").unwrap(),
                DataMappingV1::parse_json(br#"{"properties":{"body":{"type":"text"}}}"#).unwrap(),
                vec![DataDocumentV1::parse_source(
                    DocumentId::from_str("doc-b").unwrap(),
                    br#"{"body":"Beta."}"#,
                )
                .unwrap()],
            )
            .unwrap(),
        );
    }
    let catalog = CatalogInputV1::new(
        CatalogMappingV1::parse_json(
            br#"{"properties":{"canonical":{"type":"keyword","index":false}}}"#,
        )
        .unwrap(),
        vec![CatalogWrapperV1::parse_public_source(
            WrapperId::from_str("wrap-1").unwrap(),
            br#"{"path":"alpha.md","kind":"file"}"#,
        )
        .unwrap()],
    )
    .unwrap();
    let nodes = if route_count == 2 {
        vec![
            LogicalNodeRowV1::parse_json(
                br#"{"source_index":"life-docs","logical_node_id":"doc-a","title":"Alpha","preview":null,"path":"alpha.md"}"#,
            )
            .unwrap(),
            LogicalNodeRowV1::parse_json(
                br#"{"source_index":"life-notes","logical_node_id":"doc-b","title":"Beta","preview":null,"path":"beta.md"}"#,
            )
            .unwrap(),
        ]
    } else {
        Vec::new()
    };
    let graph = GraphInputV1::new(
        BrainName::from_str("life").unwrap(),
        ExtractorIdentity::from_str("extractor@1").unwrap(),
        ExtractorConfigV1::parse_json(br#"{}"#).unwrap(),
        GraphEdgeMappingV1::parse_json(br#"{"properties":{"physical_id":{"type":"keyword"}}}"#)
            .unwrap(),
        GraphNodeMappingV1::parse_json(br#"{"properties":{"physical_id":{"type":"keyword"}}}"#)
            .unwrap(),
        Vec::new(),
        nodes,
    )
    .unwrap();
    let prepared = PreparedCorpusV1::prepare(
        PrepareCorpusInputV1::new(
            RootIdentity::from_str("/r").unwrap(),
            CorpusPrefix::from_str("life").unwrap(),
            CorpusIncarnationSeed::from_array(std::array::from_fn(|i| i as u8)),
            manifest,
            data,
            catalog,
            graph,
        )
        .unwrap(),
    )
    .unwrap();
    let planned = PlannedCorpusV1::plan(
        prepared,
        SequenceTransitionV1::new(Sequence::new(0), Sequence::new(1)).unwrap(),
        Generation::new(1),
    )
    .unwrap();
    let owner = planned.desired_plan().owner().clone();
    DurableBeginBundleV1::build(ExpectedPublicationV1::absent(owner), planned).unwrap()
}

fn planned_fixture_with(
    expected: u64,
    desired: u64,
    generation: u64,
    data_source: &[u8],
    catalog_source: &[u8],
    extractor_config: &[u8],
) -> PlannedCorpusV1 {
    let manifest = ManifestV1::parse_json(br#"{"entries":[{"id":"doc-a","path":"alpha.md"},{"id":"doc-b","path":"beta.md"}],"format_version":1,"root_identity":"/r"}"#).unwrap();
    let data = DataRouteInputV1::new(
        DataSlug::from_str("docs").unwrap(),
        LogicalIndexName::from_str("life-docs").unwrap(),
        DataMappingV1::parse_json(
            br#"{"properties":{"body":{"type":"text"},"path":{"type":"keyword"}}}"#,
        )
        .unwrap(),
        vec![
            DataDocumentV1::parse_source(
                DocumentId::from_str("doc-b").unwrap(),
                br#"{"path":"beta.md","body":"Beta."}"#,
            )
            .unwrap(),
            DataDocumentV1::parse_source(DocumentId::from_str("doc-a").unwrap(), data_source)
                .unwrap(),
        ],
    )
    .unwrap();
    let catalog = CatalogInputV1::new(
        CatalogMappingV1::parse_json(
            br#"{"properties":{"canonical":{"type":"keyword","index":false}}}"#,
        )
        .unwrap(),
        vec![CatalogWrapperV1::parse_public_source(
            WrapperId::from_str("wrap-1").unwrap(),
            catalog_source,
        )
        .unwrap()],
    )
    .unwrap();
    let edge = LogicalEdgeRowV1::parse_json(br#"{"src":"doc-a","dst":"doc-b","type":"wikilink","weight":1,"confidence":1,"valid_at":0,"created_at":0,"detector":"wikilink@2","schema_version":1,"src_file":"alpha.md","evidence":{"quote":"[[beta]]","source":"alpha.md","offset":0}}"#).unwrap();
    let nodes = vec![
        LogicalNodeRowV1::parse_json(br#"{"source_index":"life-docs","logical_node_id":"doc-b","title":null,"preview":"Beta.","path":"beta.md"}"#).unwrap(),
        LogicalNodeRowV1::parse_json(br#"{"source_index":"life-docs","logical_node_id":"doc-a","title":"Alpha","preview":null,"path":"alpha.md"}"#).unwrap(),
    ];
    let graph = GraphInputV1::new(
        BrainName::from_str("life").unwrap(),
        ExtractorIdentity::from_str("extractor@1").unwrap(),
        ExtractorConfigV1::parse_json(extractor_config).unwrap(),
        GraphEdgeMappingV1::parse_json(br#"{"properties":{"physical_id":{"type":"keyword"}}}"#)
            .unwrap(),
        GraphNodeMappingV1::parse_json(br#"{"properties":{"physical_id":{"type":"keyword"}}}"#)
            .unwrap(),
        vec![edge],
        nodes,
    )
    .unwrap();
    let input = PrepareCorpusInputV1::new(
        RootIdentity::from_str("/r").unwrap(),
        CorpusPrefix::from_str("life").unwrap(),
        CorpusIncarnationSeed::from_array(std::array::from_fn(|i| i as u8)),
        manifest,
        vec![data],
        catalog,
        graph,
    )
    .unwrap();
    let prepared = PreparedCorpusV1::prepare(input).unwrap();
    PlannedCorpusV1::plan(
        prepared,
        SequenceTransitionV1::new(Sequence::new(expected), Sequence::new(desired)).unwrap(),
        Generation::new(generation),
    )
    .unwrap()
}
