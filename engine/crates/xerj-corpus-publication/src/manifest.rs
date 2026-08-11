use crate::{
    canonical_json,
    codec::Encoder,
    digest::ManifestDigest,
    error::{error, ProtocolError, ProtocolErrorKind},
    scalar::{DocumentId, RootIdentity},
};
use std::{collections::HashSet, fmt, str::FromStr};

pub struct ManifestJsonBytes(Box<[u8]>);
impl ManifestJsonBytes {
    pub fn canonical_json(&self) -> &[u8] {
        &self.0
    }
}
impl PartialEq for ManifestJsonBytes {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl Eq for ManifestJsonBytes {}
impl fmt::Debug for ManifestJsonBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ManifestJsonBytes")
            .field("len", &self.0.len())
            .finish()
    }
}

struct ManifestEntry {
    id: DocumentId,
}

pub struct ManifestV1 {
    canonical: ManifestJsonBytes,
    digest: ManifestDigest,
    root_identity: RootIdentity,
    entries: Vec<ManifestEntry>,
}

impl fmt::Debug for ManifestV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ManifestV1")
            .field("entries", &self.entries.len())
            .finish_non_exhaustive()
    }
}

impl ManifestV1 {
    pub fn parse_json(input: &[u8]) -> Result<Self, ProtocolError> {
        let value = canonical_json::parse(input, "manifest")?;
        let fields = canonical_json::closed(
            &value,
            "manifest",
            &["entries", "format_version", "root_identity"],
        )?;
        if canonical_json::u64(fields[1], "manifest.format_version")? != 1 {
            return Err(error(
                ProtocolErrorKind::InvalidVersion,
                "manifest.format_version must equal 1",
            ));
        }
        let root_identity =
            RootIdentity::from_str(canonical_json::string(fields[2], "manifest.root_identity")?)?;
        let mut entries = Vec::new();
        let mut ids = HashSet::new();
        for (index, value) in canonical_json::array(fields[0], "manifest.entries")?
            .iter()
            .enumerate()
        {
            let fields = canonical_json::closed(value, "manifest.entries[]", &["id", "path"])?;
            let id =
                DocumentId::from_str(canonical_json::string(fields[0], "manifest.entries[].id")?)?;
            let path = canonical_json::string(fields[1], "manifest.entries[].path")?;
            if path.is_empty() || path.as_bytes().contains(&0) {
                return Err(error(
                    ProtocolErrorKind::InvalidScalar,
                    format_args!("manifest.entries[{index}].path is invalid"),
                ));
            }
            if !ids.insert(id.as_protocol_str().to_owned()) {
                return Err(error(
                    ProtocolErrorKind::DuplicateTuple,
                    "manifest contains duplicate document id",
                ));
            }
            entries.push(ManifestEntry { id });
        }
        let canonical = canonical_json::canonicalize(&value);
        let mut encoded = Encoder::domain(b"xerj-autoindex-manifest-v1\0");
        encoded.bytes(&canonical);
        let digest = ManifestDigest::from_preimage(&encoded.finish());
        Ok(Self {
            canonical: ManifestJsonBytes(canonical.into_boxed_slice()),
            digest,
            root_identity,
            entries,
        })
    }
    pub fn canonical_json(&self) -> &ManifestJsonBytes {
        &self.canonical
    }
    pub fn digest(&self) -> &ManifestDigest {
        &self.digest
    }
    pub fn root_identity(&self) -> &RootIdentity {
        &self.root_identity
    }
    pub(crate) fn contains_document(&self, id: &DocumentId) -> bool {
        self.entries.iter().any(|entry| entry.id == *id)
    }
}
