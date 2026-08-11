use crate::error::{error, ProtocolError, ProtocolErrorKind, Result};
use std::{fmt, str::FromStr};

fn validate_text(field: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(error(
            ProtocolErrorKind::InvalidScalar,
            format_args!("{field} must not be empty"),
        ));
    }
    if value.as_bytes().contains(&0) {
        return Err(error(
            ProtocolErrorKind::InvalidScalar,
            format_args!("{field} contains NUL"),
        ));
    }
    Ok(())
}

macro_rules! string_scalar {
    ($name:ident, $label:literal) => {
        #[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
        pub struct $name(Box<str>);

        impl $name {
            pub fn as_protocol_str(&self) -> &str {
                &self.0
            }
        }

        impl FromStr for $name {
            type Err = ProtocolError;
            fn from_str(value: &str) -> Result<Self> {
                validate_text($label, value)?;
                Ok(Self(value.into()))
            }
        }

        impl TryFrom<String> for $name {
            type Error = ProtocolError;
            fn try_from(value: String) -> Result<Self> {
                validate_text($label, &value)?;
                Ok(Self(value.into_boxed_str()))
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

string_scalar!(RootIdentity, "root_identity");
string_scalar!(CorpusPrefix, "prefix");
string_scalar!(DataSlug, "slug");
string_scalar!(LogicalIndexName, "logical_index");
string_scalar!(BrainName, "brain");
string_scalar!(ExtractorIdentity, "extractor_identity");
string_scalar!(DocumentId, "document_id");
string_scalar!(WrapperId, "wrapper_id");

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct LogicalEdgeId(Box<str>);

impl LogicalEdgeId {
    pub fn as_lower_hex(&self) -> &str {
        &self.0
    }
    pub(crate) fn from_u128(value: u128) -> Self {
        Self(format!("{value:032x}").into_boxed_str())
    }
}

impl FromStr for LogicalEdgeId {
    type Err = ProtocolError;
    fn from_str(value: &str) -> Result<Self> {
        if value.len() != 32
            || !value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(error(
                ProtocolErrorKind::InvalidRenderedIdentity,
                "logical_edge_id must be 32 lowercase hexadecimal characters",
            ));
        }
        Ok(Self(value.into()))
    }
}

impl fmt::Display for LogicalEdgeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct PhysicalDataName(Box<str>);

impl PhysicalDataName {
    pub fn as_protocol_str(&self) -> &str {
        &self.0
    }
    pub(crate) fn from_generated(value: String) -> Result<Self> {
        validate_physical_data_name(&value)?;
        Ok(Self(value.into_boxed_str()))
    }
}

impl FromStr for PhysicalDataName {
    type Err = ProtocolError;
    fn from_str(value: &str) -> Result<Self> {
        validate_physical_data_name(value)?;
        Ok(Self(value.into()))
    }
}
impl TryFrom<String> for PhysicalDataName {
    type Error = ProtocolError;
    fn try_from(value: String) -> Result<Self> {
        Self::from_generated(value)
    }
}
impl fmt::Display for PhysicalDataName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn validate_physical_data_name(value: &str) -> Result<()> {
    if value.len() > 231 || !value.is_ascii() {
        return Err(error(
            ProtocolErrorKind::BoundsExceeded,
            "physical_data_name exceeds 231 ASCII bytes",
        ));
    }
    let rest = value.strip_prefix(".xerj-aidx-d-").ok_or_else(|| {
        error(
            ProtocolErrorKind::InvalidScalar,
            "physical_data_name has invalid prefix",
        )
    })?;
    let parts: Vec<_> = rest.split('-').collect();
    let valid_hex = |v: &str| {
        v.len() == 64
            && v.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    };
    let generation = parts
        .get(1)
        .and_then(|value| value.strip_prefix('g'))
        .and_then(|value| value.parse::<u64>().ok());
    if parts.len() != 4
        || !valid_hex(parts[0])
        || generation.is_none()
        || format!("g{}", generation.unwrap()) != parts[1]
        || !parts[2].starts_with('s')
        || !valid_hex(&parts[2][1..])
        || !parts[3].starts_with('t')
        || !valid_hex(&parts[3][1..])
    {
        return Err(error(
            ProtocolErrorKind::InvalidScalar,
            "physical_data_name has invalid hidden-name grammar",
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ResourceKey(Box<str>);

impl ResourceKey {
    pub fn as_protocol_str(&self) -> &str {
        &self.0
    }
    pub(crate) fn from_generated(value: String) -> Result<Self> {
        validate_resource_key(&value)?;
        Ok(Self(value.into_boxed_str()))
    }
}
impl FromStr for ResourceKey {
    type Err = ProtocolError;
    fn from_str(value: &str) -> Result<Self> {
        validate_resource_key(value)?;
        Ok(Self(value.into()))
    }
}
impl TryFrom<String> for ResourceKey {
    type Error = ProtocolError;
    fn try_from(value: String) -> Result<Self> {
        Self::from_generated(value)
    }
}
impl fmt::Display for ResourceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn validate_resource_key(value: &str) -> Result<()> {
    if value.as_bytes().contains(&0) {
        return Err(error(
            ProtocolErrorKind::InvalidScalar,
            "resource_key contains NUL",
        ));
    }
    if value.len() > 1024 || !value.is_ascii() {
        return Err(error(
            ProtocolErrorKind::BoundsExceeded,
            "resource_key exceeds 1024 ASCII bytes",
        ));
    }
    let parts: Vec<_> = value.split('/').collect();
    let valid = match parts.as_slice() {
        ["data", name] => PhysicalDataName::from_str(name).is_ok(),
        ["catalog", name, generation] => {
            *name == ".xerj-autoindex-catalog-generations-v1"
                && crate::digest::is_rendered(generation, "xerg1-sha256-")
        }
        ["graph-edge", name, token] => {
            name.strip_prefix(".xerj-memory-")
                .and_then(|name| name.strip_suffix("-edges"))
                .is_some_and(|brain| !brain.is_empty())
                && crate::digest::is_rendered(token, "xergt1-sha256-")
        }
        ["graph-node", name, token] => {
            *name == ".xerj-autoindex-graph-nodes-v1"
                && crate::digest::is_rendered(token, "xergt1-sha256-")
        }
        _ => false,
    };
    if !valid {
        return Err(error(
            ProtocolErrorKind::InvalidScalar,
            "resource_key has invalid closed grammar",
        ));
    }
    Ok(())
}

macro_rules! integer_scalar {
    ($name:ident) => {
        #[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
        pub struct $name(u64);
        impl $name {
            pub fn new(value: u64) -> Self {
                Self(value)
            }
            pub fn get(self) -> u64 {
                self.0
            }
        }
    };
}
integer_scalar!(Sequence);
integer_scalar!(Generation);

pub struct CorpusIncarnationSeed([u8; 32]);
impl CorpusIncarnationSeed {
    pub fn from_array(value: [u8; 32]) -> Self {
        Self(value)
    }
    pub(crate) fn consume(self) -> [u8; 32] {
        self.0
    }
}
