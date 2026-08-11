use crate::{
    codec::Encoder,
    digest::{
        CatalogGenerationIncarnationId, CatalogProjectionDigest, CorpusIncarnationId,
        CorpusOwnerId, EdgePhysicalId, GenerationId, GraphCoreDigest, GraphToken, ManifestDigest,
        NodePhysicalId, PreparedInputDigest, TransactionId,
    },
    error::Result,
    scalar::{CorpusPrefix, DataSlug, Generation, PhysicalDataName, RootIdentity, Sequence},
};

pub(crate) fn owner(root: &RootIdentity, prefix: &CorpusPrefix) -> CorpusOwnerId {
    let mut bytes = Encoder::domain(b"xerj-corpus-owner-v1\0");
    bytes.string(root.as_protocol_str());
    bytes.string(prefix.as_protocol_str());
    CorpusOwnerId::from_preimage(&bytes.finish())
}

pub(crate) fn corpus_incarnation(owner: &CorpusOwnerId, seed: [u8; 32]) -> CorpusIncarnationId {
    let mut bytes = Encoder::domain(b"xerj-corpus-incarnation-v1\0");
    bytes.string(owner.as_rendered_str());
    bytes.raw(&seed);
    CorpusIncarnationId::from_preimage(&bytes.finish())
}

pub(crate) fn transaction(
    owner: &CorpusOwnerId,
    incarnation: &CorpusIncarnationId,
    expected: Sequence,
    desired: Sequence,
    manifest: &ManifestDigest,
    prepared: &PreparedInputDigest,
) -> TransactionId {
    let mut bytes = Encoder::domain(b"xerj-autoindex-transaction-v1\0");
    bytes.string(owner.as_rendered_str());
    bytes.string(incarnation.as_rendered_str());
    bytes.u64(expected.get());
    bytes.u64(desired.get());
    bytes.string(manifest.as_rendered_str());
    bytes.string(prepared.as_rendered_str());
    TransactionId::from_preimage(&bytes.finish())
}

pub(crate) fn generation_id(
    owner: &CorpusOwnerId,
    incarnation: &CorpusIncarnationId,
    generation: Generation,
    tx: &TransactionId,
) -> GenerationId {
    let mut bytes = Encoder::domain(b"xerj-autoindex-generation-v1\0");
    bytes.string(owner.as_rendered_str());
    bytes.string(incarnation.as_rendered_str());
    bytes.u64(generation.get());
    bytes.string(tx.as_rendered_str());
    GenerationId::from_preimage(&bytes.finish())
}

pub(crate) fn graph_token(
    owner: &CorpusOwnerId,
    incarnation: &CorpusIncarnationId,
    generation: Generation,
    tx: &TransactionId,
    core: &GraphCoreDigest,
) -> GraphToken {
    let mut bytes = Encoder::domain(b"xerj-autoindex-graph-token-v1\0");
    bytes.string(owner.as_rendered_str());
    bytes.string(incarnation.as_rendered_str());
    bytes.u64(generation.get());
    bytes.string(tx.as_rendered_str());
    bytes.string(core.as_rendered_str());
    GraphToken::from_preimage(&bytes.finish())
}

pub(crate) fn catalog_incarnation(
    owner: &CorpusOwnerId,
    incarnation: &CorpusIncarnationId,
    generation: Generation,
    tx: &TransactionId,
    projection: &CatalogProjectionDigest,
) -> CatalogGenerationIncarnationId {
    let mut bytes = Encoder::domain(b"xerj-catalog-generation-incarnation-v1\0");
    bytes.string(owner.as_rendered_str());
    bytes.string(incarnation.as_rendered_str());
    bytes.u64(generation.get());
    bytes.string(tx.as_rendered_str());
    bytes.string(projection.as_rendered_str());
    CatalogGenerationIncarnationId::from_preimage(&bytes.finish())
}

pub(crate) fn physical_data_name(
    owner: &CorpusOwnerId,
    incarnation: &CorpusIncarnationId,
    tx: &TransactionId,
    manifest: &ManifestDigest,
    generation: Generation,
    slug: &DataSlug,
) -> Result<PhysicalDataName> {
    let mut owner_bytes = Encoder::domain(b"xerj-autoindex-physical-owner-v1\0");
    owner_bytes.string(owner.as_rendered_str());
    let owner_hex = crate::digest::sha256_hex(&owner_bytes.finish());
    let mut slug_bytes = Encoder::domain(b"xerj-autoindex-physical-slug-v1\0");
    slug_bytes.string(slug.as_protocol_str());
    let slug_hex = crate::digest::sha256_hex(&slug_bytes.finish());
    let mut stage_bytes = Encoder::domain(b"xerj-autoindex-stage-identity-v1\0");
    stage_bytes.string(owner.as_rendered_str());
    stage_bytes.string(incarnation.as_rendered_str());
    stage_bytes.string(tx.as_rendered_str());
    stage_bytes.string(manifest.as_rendered_str());
    stage_bytes.u64(generation.get());
    stage_bytes.string(slug.as_protocol_str());
    let stage_hex = crate::digest::sha256_hex(&stage_bytes.finish());
    PhysicalDataName::from_generated(format!(
        ".xerj-aidx-d-{owner_hex}-g{}-s{slug_hex}-t{stage_hex}",
        generation.get()
    ))
}

pub(crate) fn edge_physical_id(
    owner: &CorpusOwnerId,
    incarnation: &CorpusIncarnationId,
    generation: Generation,
    token: &GraphToken,
    logical_id: &str,
) -> EdgePhysicalId {
    let mut bytes = Encoder::domain(b"xerj-graph-edge-physical-id-v1\0");
    bytes.string(owner.as_rendered_str());
    bytes.string(incarnation.as_rendered_str());
    bytes.u64(generation.get());
    bytes.string(token.as_rendered_str());
    bytes.string(logical_id);
    EdgePhysicalId::from_preimage(&bytes.finish())
}

pub(crate) fn node_physical_id(
    owner: &CorpusOwnerId,
    incarnation: &CorpusIncarnationId,
    generation: Generation,
    token: &GraphToken,
    source_index: &str,
    logical_id: &str,
) -> NodePhysicalId {
    let mut bytes = Encoder::domain(b"xerj-graph-node-physical-id-v1\0");
    bytes.string(owner.as_rendered_str());
    bytes.string(incarnation.as_rendered_str());
    bytes.u64(generation.get());
    bytes.string(token.as_rendered_str());
    bytes.string(source_index);
    bytes.string(logical_id);
    NodePhysicalId::from_preimage(&bytes.finish())
}
