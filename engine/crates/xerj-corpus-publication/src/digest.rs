use crate::error::{error, ProtocolError, ProtocolErrorKind, Result};
use sha2::{Digest, Sha256};
use std::{fmt, str::FromStr};

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        use fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

pub(crate) fn is_rendered(value: &str, prefix: &str) -> bool {
    let Some(hex) = value.strip_prefix(prefix) else {
        return false;
    };
    hex.len() == 64
        && hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

macro_rules! branded_digest {
    ($vis:vis $name:ident, $prefix:literal) => {
        #[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
        $vis struct $name(Box<str>);

        #[allow(dead_code)]
        impl $name {
            pub fn as_rendered_str(&self) -> &str { &self.0 }
            pub(crate) fn from_preimage(bytes: &[u8]) -> Self {
                Self(format!("{}{}", $prefix, sha256_hex(bytes)).into_boxed_str())
            }
        }

        impl FromStr for $name {
            type Err = ProtocolError;
            fn from_str(value: &str) -> Result<Self> {
                if !is_rendered(value, $prefix) {
                    return Err(error(ProtocolErrorKind::InvalidRenderedIdentity,
                        concat!(stringify!($name), " has invalid rendered identity")));
                }
                Ok(Self(value.into()))
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.0) }
        }
    };
}

branded_digest!(pub CorpusOwnerId, "xercpo1-sha256-");
branded_digest!(pub CorpusIncarnationId, "xercpi1-sha256-");
branded_digest!(pub ManifestDigest, "xerm1-sha256-");
branded_digest!(pub ExtractorConfigDigest, "xerecfg1-sha256-");
branded_digest!(pub MappingDigest, "xermap1-sha256-");
branded_digest!(pub PreparedInputDigest, "xerpdi1-sha256-");
branded_digest!(pub TransactionId, "xertx1-sha256-");
branded_digest!(pub ReplayArtifactDigest, "xerra1-sha256-");
branded_digest!(pub ReplaySetDigest, "xerrs1-sha256-");
branded_digest!(pub DesiredPlanDigest, "xerdp1-sha256-");
branded_digest!(pub PublicationDigest, "xercp1-sha256-");
branded_digest!(pub ExpectedPublicationDigest, "xerep1-sha256-");
branded_digest!(pub SyncBeginDigest, "xersb1-sha256-");

branded_digest!(pub(crate) ProducerId, "xerp1-sha256-");
branded_digest!(pub(crate) DataIdDigest, "xerids1-sha256-");
branded_digest!(pub(crate) DataContentDigest, "xerdc1-sha256-");
branded_digest!(pub(crate) CatalogIdDigest, "xercids1-sha256-");
branded_digest!(pub(crate) CatalogWrapperDigest, "xercws1-sha256-");
branded_digest!(pub(crate) LogicalEdgeSetDigest, "xergle1-sha256-");
branded_digest!(pub(crate) LogicalNodeSetDigest, "xergln1-sha256-");
branded_digest!(pub(crate) GraphCoreDigest, "xergpc1-sha256-");
branded_digest!(pub(crate) GenerationId, "xerg1-sha256-");
branded_digest!(pub(crate) DataProjectionDigest, "xerd1-sha256-");
branded_digest!(pub(crate) CatalogProjectionDigest, "xercatp1-sha256-");
branded_digest!(pub(crate) GraphProjectionDigest, "xergp1-sha256-");
branded_digest!(pub(crate) CatalogGenerationIncarnationId, "xercati1-sha256-");
branded_digest!(pub(crate) GraphToken, "xergt1-sha256-");
branded_digest!(pub(crate) EdgePhysicalId, "xerge1-sha256-");
branded_digest!(pub(crate) NodePhysicalId, "xergn1-sha256-");
branded_digest!(pub(crate) EdgePhysicalIdSetDigest, "xergepi1-sha256-");
branded_digest!(pub(crate) NodePhysicalIdSetDigest, "xergnpi1-sha256-");
branded_digest!(pub(crate) StorageIncarnation, "xersi1-sha256-");
branded_digest!(pub(crate) DataSealDigest, "xerds1-sha256-");
branded_digest!(pub(crate) CatalogSealDigest, "xercs1-sha256-");
branded_digest!(pub(crate) EdgeSealDigest, "xerges1-sha256-");
branded_digest!(pub(crate) NodeSealDigest, "xergns1-sha256-");
