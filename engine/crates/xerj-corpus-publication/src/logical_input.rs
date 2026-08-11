use crate::{
    canonical_json::{self, JsonValue},
    codec::Encoder,
    digest::{ExtractorConfigDigest, MappingDigest},
    error::{error, ProtocolError, ProtocolErrorKind, Result},
    manifest::ManifestV1,
    scalar::{
        BrainName, CorpusIncarnationSeed, CorpusPrefix, DataSlug, DocumentId, ExtractorIdentity,
        LogicalEdgeId, LogicalIndexName, RootIdentity, WrapperId,
    },
};
use std::{collections::HashSet, fmt, str::FromStr};
use xxhash_rust::xxh3::xxh3_128;

#[derive(Clone)]
pub(crate) struct OpenJson {
    pub(crate) canonical: Box<[u8]>,
}

fn parse_open(input: &[u8], field: &str, require_object: bool) -> Result<OpenJson> {
    let value = canonical_json::parse(input, field)?;
    if require_object {
        canonical_json::object(&value, field)?;
    }
    let canonical = canonical_json::canonicalize(&value).into_boxed_slice();
    Ok(OpenJson { canonical })
}

macro_rules! mapping_type {
    ($name:ident) => {
        pub struct $name {
            pub(crate) json: OpenJson,
            pub(crate) digest: MappingDigest,
        }
        impl $name {
            pub fn parse_json(input: &[u8]) -> Result<Self, ProtocolError> {
                let json = parse_open(input, stringify!($name), true)?;
                let mut bytes = Encoder::domain(b"xerj-mapping-v1\0");
                bytes.bytes(&json.canonical);
                let digest = MappingDigest::from_preimage(&bytes.finish());
                Ok(Self { json, digest })
            }
            pub fn digest(&self) -> &MappingDigest {
                &self.digest
            }
        }
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct(stringify!($name))
                    .field("canonical_len", &self.json.canonical.len())
                    .finish()
            }
        }
    };
}
mapping_type!(DataMappingV1);
mapping_type!(CatalogMappingV1);
mapping_type!(GraphEdgeMappingV1);
mapping_type!(GraphNodeMappingV1);

pub struct ExtractorConfigV1 {
    pub(crate) json: OpenJson,
    pub(crate) digest: ExtractorConfigDigest,
}
impl ExtractorConfigV1 {
    pub fn parse_json(input: &[u8]) -> Result<Self, ProtocolError> {
        let json = parse_open(input, "extractor_config", false)?;
        let digest = ExtractorConfigDigest::from_preimage(&json.canonical);
        Ok(Self { json, digest })
    }
    pub fn digest(&self) -> &ExtractorConfigDigest {
        &self.digest
    }
}
impl fmt::Debug for ExtractorConfigV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExtractorConfigV1")
            .field("canonical_len", &self.json.canonical.len())
            .finish()
    }
}

pub struct DataDocumentV1 {
    pub(crate) id: DocumentId,
    pub(crate) source: OpenJson,
}
impl DataDocumentV1 {
    pub fn parse_source(
        id: DocumentId,
        canonicalizable_json: &[u8],
    ) -> Result<Self, ProtocolError> {
        Ok(Self {
            id,
            source: parse_open(canonicalizable_json, "data_document.source", true)?,
        })
    }
    pub fn id(&self) -> &DocumentId {
        &self.id
    }
}
impl fmt::Debug for DataDocumentV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DataDocumentV1")
            .field("id", &self.id)
            .field("source_len", &self.source.canonical.len())
            .finish()
    }
}

pub struct CatalogWrapperV1 {
    pub(crate) id: WrapperId,
    pub(crate) source: OpenJson,
}
impl CatalogWrapperV1 {
    pub fn parse_public_source(
        id: WrapperId,
        canonicalizable_json: &[u8],
    ) -> Result<Self, ProtocolError> {
        Ok(Self {
            id,
            source: parse_open(canonicalizable_json, "catalog_wrapper.public_source", true)?,
        })
    }
    pub fn id(&self) -> &WrapperId {
        &self.id
    }
}
impl fmt::Debug for CatalogWrapperV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CatalogWrapperV1")
            .field("id", &self.id)
            .field("source_len", &self.source.canonical.len())
            .finish()
    }
}

pub struct LogicalEdgeRowV1 {
    pub(crate) canonical: Box<[u8]>,
    pub(crate) logical_id: LogicalEdgeId,
    pub(crate) value: JsonValue,
}

impl LogicalEdgeRowV1 {
    pub fn parse_json(input: &[u8]) -> Result<Self, ProtocolError> {
        let value = canonical_json::parse(input, "logical_edge")?;
        let entries = canonical_json::object(&value, "logical_edge")?;
        const REQUIRED: &[&str] = &[
            "src",
            "dst",
            "type",
            "weight",
            "confidence",
            "valid_at",
            "created_at",
            "detector",
            "schema_version",
            "src_file",
            "evidence",
        ];
        const OPTIONAL: &[&str] = &["src_format", "dst_format"];
        if entries.len() < REQUIRED.len()
            || entries.len() > REQUIRED.len() + OPTIONAL.len()
            || entries.iter().any(|(key, _)| {
                !REQUIRED.contains(&key.as_str()) && !OPTIONAL.contains(&key.as_str())
            })
            || REQUIRED
                .iter()
                .any(|name| !entries.iter().any(|(key, _)| key == name))
        {
            return Err(error(
                ProtocolErrorKind::InvalidJson,
                "logical_edge has missing or unknown members",
            ));
        }
        let get = |name| canonical_json::member(&value, "logical_edge", name);
        let src = nonempty(
            canonical_json::string(get("src")?, "logical_edge.src")?,
            "logical_edge.src",
        )?;
        let dst = nonempty(
            canonical_json::string(get("dst")?, "logical_edge.dst")?,
            "logical_edge.dst",
        )?;
        let edge_type = nonempty(
            canonical_json::string(get("type")?, "logical_edge.type")?,
            "logical_edge.type",
        )?;
        if src == dst {
            return Err(error(
                ProtocolErrorKind::CrossFieldMismatch,
                "logical_edge src must differ from dst",
            ));
        }
        let _ = canonical_json::finite_number(get("weight")?, "logical_edge.weight")?;
        let _ = canonical_json::finite_number(get("confidence")?, "logical_edge.confidence")?;
        let valid_at = canonical_json::i64(get("valid_at")?, "logical_edge.valid_at")?;
        let _ = canonical_json::i64(get("created_at")?, "logical_edge.created_at")?;
        nonempty(
            canonical_json::string(get("detector")?, "logical_edge.detector")?,
            "logical_edge.detector",
        )?;
        let schema = canonical_json::u64(get("schema_version")?, "logical_edge.schema_version")?;
        u32::try_from(schema).map_err(|_| {
            error(
                ProtocolErrorKind::BoundsExceeded,
                "logical_edge.schema_version exceeds u32",
            )
        })?;
        let src_file = nonempty(
            canonical_json::string(get("src_file")?, "logical_edge.src_file")?,
            "logical_edge.src_file",
        )?;
        for optional in OPTIONAL {
            if let Ok(value) = get(optional) {
                let _ = canonical_json::string(value, "logical_edge optional format")?;
            }
        }
        let evidence = get("evidence")?;
        let evidence_fields = canonical_json::closed(
            evidence,
            "logical_edge.evidence",
            &["quote", "source", "offset"],
        )?;
        let quote = canonical_json::string(evidence_fields[0], "logical_edge.evidence.quote")?;
        if quote.chars().count() > 240 {
            return Err(error(
                ProtocolErrorKind::BoundsExceeded,
                "logical_edge evidence quote exceeds 240 Unicode scalar values",
            ));
        }
        if canonical_json::string(evidence_fields[1], "logical_edge.evidence.source")? != src_file {
            return Err(error(
                ProtocolErrorKind::CrossFieldMismatch,
                "logical_edge evidence source must equal src_file",
            ));
        }
        let _ = canonical_json::u64(evidence_fields[2], "logical_edge.evidence.offset")?;
        let mut identity = Vec::new();
        identity.extend_from_slice(b"xg1\0");
        identity.extend_from_slice(src.as_bytes());
        identity.push(0);
        identity.extend_from_slice(edge_type.as_bytes());
        identity.push(0);
        identity.extend_from_slice(dst.as_bytes());
        identity.push(0);
        identity.extend_from_slice(valid_at.to_string().as_bytes());
        let logical_id = LogicalEdgeId::from_u128(xxh3_128(&identity));
        let canonical = canonical_json::canonicalize(&value).into_boxed_slice();
        Ok(Self {
            canonical,
            logical_id,
            value,
        })
    }
    pub fn logical_id(&self) -> &LogicalEdgeId {
        &self.logical_id
    }
}
impl fmt::Debug for LogicalEdgeRowV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LogicalEdgeRowV1")
            .field("logical_id", &self.logical_id)
            .finish_non_exhaustive()
    }
}

pub struct LogicalNodeRowV1 {
    pub(crate) canonical: Box<[u8]>,
    pub(crate) source_index: Box<str>,
    pub(crate) logical_node_id: Box<str>,
    pub(crate) value: JsonValue,
}
impl LogicalNodeRowV1 {
    pub fn parse_json(input: &[u8]) -> Result<Self, ProtocolError> {
        let value = canonical_json::parse(input, "logical_node")?;
        let fields = canonical_json::closed(
            &value,
            "logical_node",
            &[
                "source_index",
                "logical_node_id",
                "title",
                "preview",
                "path",
            ],
        )?;
        let source_index = nonempty(
            canonical_json::string(fields[0], "logical_node.source_index")?,
            "logical_node.source_index",
        )?;
        let logical_node_id = nonempty(
            canonical_json::string(fields[1], "logical_node.logical_node_id")?,
            "logical_node.logical_node_id",
        )?;
        let _ = canonical_json::nullable_string(fields[2], "logical_node.title")?;
        let _ = canonical_json::nullable_string(fields[3], "logical_node.preview")?;
        nonempty(
            canonical_json::string(fields[4], "logical_node.path")?,
            "logical_node.path",
        )?;
        let canonical = canonical_json::canonicalize(&value).into_boxed_slice();
        Ok(Self {
            canonical,
            source_index: source_index.into(),
            logical_node_id: logical_node_id.into(),
            value,
        })
    }
}
impl fmt::Debug for LogicalNodeRowV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LogicalNodeRowV1")
            .field("source_index", &self.source_index)
            .field("logical_node_id", &self.logical_node_id)
            .finish()
    }
}

fn nonempty<'a>(value: &'a str, field: &str) -> Result<&'a str> {
    if value.is_empty() || value.as_bytes().contains(&0) {
        Err(error(
            ProtocolErrorKind::InvalidScalar,
            format_args!("{field} must be nonempty and contain no NUL"),
        ))
    } else {
        Ok(value)
    }
}

pub struct DataRouteInputV1 {
    pub(crate) slug: DataSlug,
    pub(crate) logical_index: LogicalIndexName,
    pub(crate) mapping: DataMappingV1,
    pub(crate) documents: Vec<DataDocumentV1>,
}
impl DataRouteInputV1 {
    pub fn new(
        slug: DataSlug,
        logical_index: LogicalIndexName,
        mapping: DataMappingV1,
        mut documents: Vec<DataDocumentV1>,
    ) -> Result<Self, ProtocolError> {
        documents.sort_by(|a, b| a.id.as_protocol_str().cmp(b.id.as_protocol_str()));
        reject_adjacent_duplicate(
            documents.iter().map(|v| v.id.as_protocol_str()),
            "data route contains duplicate document id",
        )?;
        Ok(Self {
            slug,
            logical_index,
            mapping,
            documents,
        })
    }
}
impl fmt::Debug for DataRouteInputV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DataRouteInputV1")
            .field("slug", &self.slug)
            .field("document_count", &self.documents.len())
            .finish_non_exhaustive()
    }
}

pub struct CatalogInputV1 {
    pub(crate) mapping: CatalogMappingV1,
    pub(crate) wrappers: Vec<CatalogWrapperV1>,
}
impl CatalogInputV1 {
    pub fn new(
        mapping: CatalogMappingV1,
        mut wrappers: Vec<CatalogWrapperV1>,
    ) -> Result<Self, ProtocolError> {
        wrappers.sort_by(|a, b| a.id.as_protocol_str().cmp(b.id.as_protocol_str()));
        reject_adjacent_duplicate(
            wrappers.iter().map(|v| v.id.as_protocol_str()),
            "catalog contains duplicate wrapper id",
        )?;
        Ok(Self { mapping, wrappers })
    }
}
impl fmt::Debug for CatalogInputV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CatalogInputV1")
            .field("wrapper_count", &self.wrappers.len())
            .finish_non_exhaustive()
    }
}

pub struct GraphInputV1 {
    pub(crate) brain: BrainName,
    pub(crate) extractor_identity: ExtractorIdentity,
    pub(crate) extractor_config: ExtractorConfigV1,
    pub(crate) edge_mapping: GraphEdgeMappingV1,
    pub(crate) node_mapping: GraphNodeMappingV1,
    pub(crate) edges: Vec<LogicalEdgeRowV1>,
    pub(crate) nodes: Vec<LogicalNodeRowV1>,
}
impl GraphInputV1 {
    pub fn new(
        brain: BrainName,
        extractor_identity: ExtractorIdentity,
        extractor_config: ExtractorConfigV1,
        edge_mapping: GraphEdgeMappingV1,
        node_mapping: GraphNodeMappingV1,
        mut edges: Vec<LogicalEdgeRowV1>,
        mut nodes: Vec<LogicalNodeRowV1>,
    ) -> Result<Self, ProtocolError> {
        edges.sort_by(|a, b| {
            (a.logical_id.as_lower_hex(), &*a.canonical)
                .cmp(&(b.logical_id.as_lower_hex(), &*b.canonical))
        });
        reject_adjacent_duplicate(
            edges.iter().map(|v| v.logical_id.as_lower_hex()),
            "graph contains duplicate logical edge id",
        )?;
        nodes.sort_by(|a, b| {
            (a.source_index.as_ref(), a.logical_node_id.as_ref())
                .cmp(&(b.source_index.as_ref(), b.logical_node_id.as_ref()))
        });
        let mut seen = HashSet::new();
        if nodes
            .iter()
            .any(|v| !seen.insert((v.source_index.to_string(), v.logical_node_id.to_string())))
        {
            return Err(error(
                ProtocolErrorKind::DuplicateTuple,
                "graph contains duplicate logical node tuple",
            ));
        }
        Ok(Self {
            brain,
            extractor_identity,
            extractor_config,
            edge_mapping,
            node_mapping,
            edges,
            nodes,
        })
    }
}
impl fmt::Debug for GraphInputV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GraphInputV1")
            .field("brain", &self.brain)
            .field("edge_count", &self.edges.len())
            .field("node_count", &self.nodes.len())
            .finish_non_exhaustive()
    }
}

pub struct PrepareCorpusInputV1 {
    pub(crate) root_identity: RootIdentity,
    pub(crate) prefix: CorpusPrefix,
    pub(crate) corpus_seed: CorpusIncarnationSeed,
    pub(crate) manifest: ManifestV1,
    pub(crate) data: Vec<DataRouteInputV1>,
    pub(crate) catalog: CatalogInputV1,
    pub(crate) graph: GraphInputV1,
}
impl PrepareCorpusInputV1 {
    pub fn new(
        root_identity: RootIdentity,
        prefix: CorpusPrefix,
        corpus_seed: CorpusIncarnationSeed,
        manifest: ManifestV1,
        mut data: Vec<DataRouteInputV1>,
        catalog: CatalogInputV1,
        graph: GraphInputV1,
    ) -> Result<Self, ProtocolError> {
        if manifest.root_identity() != &root_identity {
            return Err(error(
                ProtocolErrorKind::CrossFieldMismatch,
                "manifest root_identity does not match prepared root_identity",
            ));
        }
        if graph.brain.as_protocol_str() != prefix.as_protocol_str() {
            return Err(error(
                ProtocolErrorKind::CrossFieldMismatch,
                "graph brain must equal corpus prefix",
            ));
        }
        for route in &data {
            let expected = format!(
                "{}-{}",
                prefix.as_protocol_str(),
                route.slug.as_protocol_str()
            );
            if route.logical_index.as_protocol_str() != expected {
                return Err(error(
                    ProtocolErrorKind::CrossFieldMismatch,
                    "logical index must equal <prefix>-<slug>",
                ));
            }
            for document in &route.documents {
                if !manifest.contains_document(&document.id) {
                    return Err(error(
                        ProtocolErrorKind::CrossFieldMismatch,
                        "data document is absent from manifest",
                    ));
                }
            }
        }
        let routes: HashSet<_> = data
            .iter()
            .map(|v| v.logical_index.as_protocol_str())
            .collect();
        for node in &graph.nodes {
            if !routes.contains(node.source_index.as_ref()) {
                return Err(error(
                    ProtocolErrorKind::CrossFieldMismatch,
                    "logical node source_index is not a data route",
                ));
            }
            if !manifest.contains_document(&DocumentId::from_str(&node.logical_node_id)?) {
                return Err(error(
                    ProtocolErrorKind::CrossFieldMismatch,
                    "logical node id is absent from manifest",
                ));
            }
        }
        data.sort_by(|a, b| a.slug.as_protocol_str().cmp(b.slug.as_protocol_str()));
        reject_adjacent_duplicate(
            data.iter().map(|v| v.slug.as_protocol_str()),
            "prepared input contains duplicate data slug",
        )?;
        let mut route_names = HashSet::new();
        if data
            .iter()
            .any(|v| !route_names.insert(v.logical_index.as_protocol_str()))
        {
            return Err(error(
                ProtocolErrorKind::DuplicateTuple,
                "prepared input contains duplicate logical route",
            ));
        }
        Ok(Self {
            root_identity,
            prefix,
            corpus_seed,
            manifest,
            data,
            catalog,
            graph,
        })
    }
}
impl fmt::Debug for PrepareCorpusInputV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PrepareCorpusInputV1")
            .field("root_identity", &self.root_identity)
            .field("prefix", &self.prefix)
            .field("routes", &self.data.len())
            .finish_non_exhaustive()
    }
}

fn reject_adjacent_duplicate<'a>(
    values: impl IntoIterator<Item = &'a str>,
    detail: &str,
) -> Result<()> {
    let mut previous = None;
    for value in values {
        if previous == Some(value) {
            return Err(error(ProtocolErrorKind::DuplicateTuple, detail));
        }
        previous = Some(value);
    }
    Ok(())
}
