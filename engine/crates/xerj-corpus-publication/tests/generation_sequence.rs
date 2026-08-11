#[path = "support/mod.rs"]
mod support;

use sha2::{Digest, Sha256};
use xerj_corpus_publication::{
    CorpusPublicationV1, ProjectionKind, ProtocolErrorKind, ReplayArtifactV1,
};

struct Encoder(Vec<u8>);

impl Encoder {
    fn domain(value: &[u8]) -> Self {
        Self(value.to_vec())
    }

    fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }

    fn string(&mut self, value: &str) {
        self.u64(value.len() as u64);
        self.0.extend_from_slice(value.as_bytes());
    }

    fn array_len(&mut self, value: usize) {
        self.u64(value as u64);
    }
}

fn rendered(prefix: &str, encoded: &Encoder) -> String {
    format!("{prefix}{:x}", Sha256::digest(&encoded.0))
}

fn string<'a>(value: &'a serde_json::Value, name: &str) -> &'a str {
    value[name].as_str().unwrap()
}

fn number(value: &serde_json::Value, name: &str) -> u64 {
    value[name].as_u64().unwrap()
}

fn graph_core_body(out: &mut Encoder, graph: &serde_json::Value) {
    out.string(string(graph, "brain"));
    out.string(string(graph, "owner"));
    out.string(string(graph, "producer"));
    out.u64(number(graph, "edge_count"));
    out.string(string(graph, "logical_edge_digest"));
    out.u64(number(graph, "node_count"));
    out.string(string(graph, "logical_node_digest"));
}

fn artifact_ids(bytes: &[u8]) -> Vec<String> {
    let lines = bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    assert_eq!(lines.len() % 2, 0);
    lines
        .chunks_exact(2)
        .map(|pair| {
            let action: serde_json::Value = serde_json::from_slice(pair[0]).unwrap();
            action["index"]["_id"].as_str().unwrap().to_owned()
        })
        .collect()
}

fn physical_id_set(domain: &[u8], prefix: &str, mut ids: Vec<String>) -> String {
    ids.sort();
    let mut encoded = Encoder::domain(domain);
    encoded.array_len(ids.len());
    for id in ids {
        encoded.string(&id);
    }
    rendered(prefix, &encoded)
}

fn artifact_for(artifacts: &[ReplayArtifactV1], projection: ProjectionKind) -> &ReplayArtifactV1 {
    let mut matches = artifacts
        .iter()
        .filter(|artifact| artifact.projection_kind() == projection);
    let artifact = matches.next().expect("projection replay artifact");
    assert!(matches.next().is_none(), "projection must be unique");
    artifact
}

#[allow(clippy::too_many_arguments)]
fn seal_digest(
    domain: &[u8],
    prefix: &str,
    publication: &serde_json::Value,
    generation: u64,
    kind: &str,
    storage_name: &str,
    storage_incarnation: &str,
    seal: &serde_json::Value,
    mapping: &str,
    count: u64,
    ids: &str,
    content: &str,
    projection: &str,
) -> String {
    let mut encoded = Encoder::domain(domain);
    encoded.string(string(publication, "owner"));
    encoded.string(string(publication, "incarnation"));
    encoded.string(string(publication, "tx_id"));
    encoded.u64(generation);
    encoded.string(kind);
    encoded.string(storage_name);
    encoded.string(storage_incarnation);
    encoded.u64(number(seal, "final_write_sequence"));
    encoded.string(mapping);
    encoded.u64(count);
    encoded.string(ids);
    encoded.string(content);
    encoded.string(projection);
    rendered(prefix, &encoded)
}

fn encode_seal(out: &mut Encoder, seal: &serde_json::Value) {
    out.u32(number(seal, "seal_version") as u32);
    out.u64(number(seal, "final_write_sequence"));
    out.string(string(seal, "seal_digest"));
}

fn publication_digest(value: &serde_json::Value) -> String {
    let data = &value["data"];
    let catalog = &value["catalog"];
    let graph = &value["graph"];
    let mut encoded = Encoder::domain(b"xerj-corpus-publication-v1\0");
    encoded.u32(number(value, "format_version") as u32);
    encoded.string(string(value, "owner"));
    encoded.string(string(value, "prefix"));
    encoded.string(string(value, "root_identity"));
    encoded.string(string(value, "incarnation"));
    encoded.u64(number(value, "sequence"));
    encoded.string(string(value, "tx_id"));
    encoded.string(string(value, "manifest_digest"));
    encoded.string(string(value, "plan_digest"));
    encoded.u64(number(data, "generation"));
    encoded.string(string(data, "projection_digest"));
    let indices = data["indices"].as_array().unwrap();
    encoded.array_len(indices.len());
    for index in indices {
        encoded.string(string(index, "slug"));
        encoded.string(string(index, "logical_index"));
        encoded.string(string(index, "physical_index"));
        encoded.string(string(index, "physical_index_incarnation"));
        encoded.string(string(index, "mapping_digest"));
        encoded.u64(number(index, "document_count"));
        encoded.string(string(index, "id_digest"));
        encoded.string(string(index, "content_digest"));
        encode_seal(&mut encoded, &index["seal"]);
    }
    encoded.string(string(catalog, "storage_index"));
    encoded.string(string(catalog, "storage_incarnation"));
    encoded.string(string(catalog, "generation_id"));
    encoded.string(string(catalog, "incarnation"));
    encoded.string(string(catalog, "mapping_digest"));
    encoded.u64(number(catalog, "document_count"));
    encoded.string(string(catalog, "id_digest"));
    encoded.string(string(catalog, "content_digest"));
    encoded.string(string(catalog, "projection_digest"));
    encode_seal(&mut encoded, &catalog["seal"]);
    encoded.string(string(graph, "brain"));
    encoded.string(string(graph, "owner"));
    encoded.u64(number(graph, "generation"));
    encoded.string(string(graph, "producer"));
    encoded.string(string(graph, "core_digest"));
    encoded.string(string(graph, "active_token"));
    encoded.string(string(graph, "edges_index"));
    encoded.string(string(graph, "edges_index_incarnation"));
    encoded.string(string(graph, "nodes_index"));
    encoded.string(string(graph, "nodes_index_incarnation"));
    encoded.string(string(graph, "edge_mapping_digest"));
    encoded.string(string(graph, "node_mapping_digest"));
    encoded.u64(number(graph, "edge_count"));
    encoded.string(string(graph, "logical_edge_digest"));
    encoded.string(string(graph, "edge_physical_id_digest"));
    encoded.u64(number(graph, "node_count"));
    encoded.string(string(graph, "logical_node_digest"));
    encoded.string(string(graph, "node_physical_id_digest"));
    encoded.string(string(graph, "projection_digest"));
    encode_seal(&mut encoded, &graph["edge_seal"]);
    encode_seal(&mut encoded, &graph["node_seal"]);
    rendered("xercp1-sha256-", &encoded)
}

fn sequence_one_generation_seven_publication() -> serde_json::Value {
    const GENERATION: u64 = 7;
    let bundle = support::absent_bundle(GENERATION);
    let artifacts = bundle.replay_artifacts();
    assert_eq!(artifacts.len(), 4);
    let mut value: serde_json::Value =
        serde_json::from_slice(include_bytes!("../testdata/review11-v1/publication.json")).unwrap();
    assert_eq!(number(&value, "sequence"), 1);
    assert_eq!(
        string(&value, "tx_id"),
        bundle.desired_plan().transaction_id().as_rendered_str()
    );
    value["plan_digest"] = bundle.desired_plan().digest().as_rendered_str().into();

    let data_artifact = artifact_for(artifacts, ProjectionKind::Data);
    let catalog_artifact = artifact_for(artifacts, ProjectionKind::Catalog);
    let edge_artifact = artifact_for(artifacts, ProjectionKind::GraphEdge);
    let node_artifact = artifact_for(artifacts, ProjectionKind::GraphNode);

    let data_resource = data_artifact.resource_key().as_protocol_str();
    let physical = data_resource.strip_prefix("data/").unwrap();
    value["data"]["generation"] = GENERATION.into();
    value["data"]["indices"][0]["physical_index"] = physical.into();
    let mut data_projection = Encoder::domain(b"xerj-data-projection-v1\0");
    data_projection.u64(GENERATION);
    let indices = value["data"]["indices"].as_array().unwrap();
    data_projection.array_len(indices.len());
    for index in indices {
        data_projection.string(string(index, "slug"));
        data_projection.string(string(index, "logical_index"));
        data_projection.string(string(index, "physical_index"));
        data_projection.string(string(index, "mapping_digest"));
        data_projection.u64(number(index, "document_count"));
        data_projection.string(string(index, "id_digest"));
        data_projection.string(string(index, "content_digest"));
    }
    value["data"]["projection_digest"] = rendered("xerd1-sha256-", &data_projection).into();

    let catalog_resource = catalog_artifact.resource_key().as_protocol_str();
    let generation_id = catalog_resource.rsplit('/').next().unwrap();
    value["catalog"]["generation_id"] = generation_id.into();
    let mut catalog_projection = Encoder::domain(b"xerj-catalog-projection-v1\0");
    catalog_projection.string(string(&value, "owner"));
    catalog_projection.string(string(&value, "incarnation"));
    catalog_projection.u64(GENERATION);
    catalog_projection.string(generation_id);
    catalog_projection.u64(number(&value["catalog"], "document_count"));
    catalog_projection.string(string(&value["catalog"], "content_digest"));
    value["catalog"]["projection_digest"] =
        rendered("xercatp1-sha256-", &catalog_projection).into();
    let mut catalog_incarnation = Encoder::domain(b"xerj-catalog-generation-incarnation-v1\0");
    catalog_incarnation.string(string(&value, "owner"));
    catalog_incarnation.string(string(&value, "incarnation"));
    catalog_incarnation.u64(GENERATION);
    catalog_incarnation.string(string(&value, "tx_id"));
    catalog_incarnation.string(string(&value["catalog"], "projection_digest"));
    value["catalog"]["incarnation"] = rendered("xercati1-sha256-", &catalog_incarnation).into();

    let graph_resource = edge_artifact.resource_key().as_protocol_str();
    let token = graph_resource.rsplit('/').next().unwrap();
    value["graph"]["generation"] = GENERATION.into();
    value["graph"]["active_token"] = token.into();
    value["graph"]["edge_physical_id_digest"] = physical_id_set(
        b"xerj-graph-edge-physical-ids-v1\0",
        "xergepi1-sha256-",
        artifact_ids(edge_artifact.bytes().artifact_bytes()),
    )
    .into();
    value["graph"]["node_physical_id_digest"] = physical_id_set(
        b"xerj-graph-node-physical-ids-v1\0",
        "xergnpi1-sha256-",
        artifact_ids(node_artifact.bytes().artifact_bytes()),
    )
    .into();
    let mut graph_projection = Encoder::domain(b"xerj-graph-projection-v1\0");
    graph_core_body(&mut graph_projection, &value["graph"]);
    graph_projection.string(string(&value["graph"], "core_digest"));
    graph_projection.u64(GENERATION);
    graph_projection.string(token);
    graph_projection.string(string(&value["graph"], "edge_physical_id_digest"));
    graph_projection.string(string(&value["graph"], "node_physical_id_digest"));
    value["graph"]["projection_digest"] = rendered("xergp1-sha256-", &graph_projection).into();

    let data = &value["data"]["indices"][0];
    let data_seal = seal_digest(
        b"xerj-data-seal-v1\0",
        "xerds1-sha256-",
        &value,
        GENERATION,
        "data",
        string(data, "physical_index"),
        string(data, "physical_index_incarnation"),
        &data["seal"],
        string(data, "mapping_digest"),
        number(data, "document_count"),
        string(data, "id_digest"),
        string(data, "content_digest"),
        string(&value["data"], "projection_digest"),
    );
    value["data"]["indices"][0]["seal"]["seal_digest"] = data_seal.into();
    let catalog = &value["catalog"];
    let catalog_seal = seal_digest(
        b"xerj-catalog-seal-v1\0",
        "xercs1-sha256-",
        &value,
        GENERATION,
        "catalog",
        string(catalog, "storage_index"),
        string(catalog, "storage_incarnation"),
        &catalog["seal"],
        string(catalog, "mapping_digest"),
        number(catalog, "document_count"),
        string(catalog, "id_digest"),
        string(catalog, "content_digest"),
        string(catalog, "projection_digest"),
    );
    value["catalog"]["seal"]["seal_digest"] = catalog_seal.into();
    let graph = &value["graph"];
    let edge_seal = seal_digest(
        b"xerj-graph-edge-seal-v1\0",
        "xerges1-sha256-",
        &value,
        GENERATION,
        "graph-edge",
        string(graph, "edges_index"),
        string(graph, "edges_index_incarnation"),
        &graph["edge_seal"],
        string(graph, "edge_mapping_digest"),
        number(graph, "edge_count"),
        string(graph, "edge_physical_id_digest"),
        string(graph, "logical_edge_digest"),
        string(graph, "projection_digest"),
    );
    let node_seal = seal_digest(
        b"xerj-graph-node-seal-v1\0",
        "xergns1-sha256-",
        &value,
        GENERATION,
        "graph-node",
        string(graph, "nodes_index"),
        string(graph, "nodes_index_incarnation"),
        &graph["node_seal"],
        string(graph, "node_mapping_digest"),
        number(graph, "node_count"),
        string(graph, "node_physical_id_digest"),
        string(graph, "logical_node_digest"),
        string(graph, "projection_digest"),
    );
    value["graph"]["edge_seal"]["seal_digest"] = edge_seal.into();
    value["graph"]["node_seal"]["seal_digest"] = node_seal.into();
    value["publication_digest"] = publication_digest(&value).into();
    value
}

#[test]
fn complete_prior_publication_allows_sequence_one_generation_seven() {
    let value = sequence_one_generation_seven_publication();
    let encoded = serde_json::to_vec(&value).unwrap();
    let parsed = CorpusPublicationV1::parse_closed_json(&encoded).unwrap();
    assert_eq!(parsed.sequence().get(), 1);
    assert!(
        std::str::from_utf8(parsed.canonical_json().canonical_json())
            .unwrap()
            .contains("\"generation\":7")
    );
}

#[test]
fn inconsistent_generation_descendant_still_rejects() {
    let mut value = sequence_one_generation_seven_publication();
    let original: serde_json::Value =
        serde_json::from_slice(include_bytes!("../testdata/review11-v1/publication.json")).unwrap();
    value["catalog"]["generation_id"] = original["catalog"]["generation_id"].clone();
    let error =
        CorpusPublicationV1::parse_closed_json(&serde_json::to_vec(&value).unwrap()).unwrap_err();
    assert_eq!(error.kind(), ProtocolErrorKind::CrossFieldMismatch);
    assert_eq!(error.to_string(), "catalog generation identity mismatch");
}
