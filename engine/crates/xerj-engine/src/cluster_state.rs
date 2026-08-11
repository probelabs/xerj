//! Closed persistence contract for `cluster_state.json` format 1.
//!
//! This module is intentionally persistence-only. Public request DTOs stay
//! forward-compatible at the API boundary; bytes that can later be rewritten
//! as durable cluster metadata are accepted only when their complete format-1
//! schema is understood.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde::de::{Error as _, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Number, Value};
use tracing::{error, warn};

use crate::engine::{DataStream, IndexTemplate};

pub const CLUSTER_STATE_V1: u32 = 1;

const DUPLICATE_KEY_MARKER: &str = "duplicate JSON object key";

/// Stable reason why this process may not rewrite cluster metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClusterStateBlockKind {
    ReadFailure,
    MalformedJson,
    DuplicateKey,
    MissingDiscriminator,
    AmbiguousDiscriminator,
    InvalidDiscriminator,
    UnsupportedFormat,
    IncompatibleSchema,
}

/// Immutable result of the one boot-time classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClusterStateBootStatus {
    Absent,
    LoadedV1,
    Blocked {
        kind: ClusterStateBlockKind,
        detail: String,
    },
}

impl ClusterStateBootStatus {
    pub fn is_writable(&self) -> bool {
        matches!(self, Self::Absent | Self::LoadedV1)
    }

    pub fn block_kind(&self) -> Option<&ClusterStateBlockKind> {
        match self {
            Self::Blocked { kind, .. } => Some(kind),
            Self::Absent | Self::LoadedV1 => None,
        }
    }

    pub(crate) fn detail(&self) -> Option<&str> {
        match self {
            Self::Blocked { detail, .. } => Some(detail),
            Self::Absent | Self::LoadedV1 => None,
        }
    }
}

#[derive(Debug)]
pub(crate) struct PreparedClusterState {
    pub(crate) status: ClusterStateBootStatus,
    pub(crate) document: Option<PersistedClusterStateV1>,
}

impl PreparedClusterState {
    fn blocked(path: &Path, kind: ClusterStateBlockKind, detail: impl Into<String>) -> Self {
        let detail = detail.into();
        error!(
            path = %path.display(),
            kind = ?kind,
            detail = %detail,
            "cluster state was rejected at boot; start a compatible XERJ build or restore supported bytes, then restart"
        );
        Self {
            status: ClusterStateBootStatus::Blocked { kind, detail },
            document: None,
        }
    }
}

/// Exact seven-field envelope emitted by the only shipped format-1 writer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PersistedClusterStateV1 {
    pub(crate) version: u32,
    pub(crate) index_templates: BTreeMap<String, PersistedIndexTemplateV1>,
    pub(crate) legacy_templates: BTreeMap<String, Value>,
    pub(crate) component_templates: BTreeMap<String, Value>,
    pub(crate) pipelines: BTreeMap<String, Value>,
    pub(crate) data_streams: BTreeMap<String, PersistedDataStreamV1>,
    pub(crate) ilm_policies: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PersistedIndexTemplateV1 {
    index_patterns: Vec<String>,
    settings: Value,
    mappings: Value,
    priority: i32,
}

impl From<IndexTemplate> for PersistedIndexTemplateV1 {
    fn from(value: IndexTemplate) -> Self {
        Self {
            index_patterns: value.index_patterns,
            settings: value.settings,
            mappings: value.mappings,
            priority: value.priority,
        }
    }
}

impl From<PersistedIndexTemplateV1> for IndexTemplate {
    fn from(value: PersistedIndexTemplateV1) -> Self {
        Self {
            index_patterns: value.index_patterns,
            settings: value.settings,
            mappings: value.mappings,
            priority: value.priority,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PersistedDataStreamV1 {
    name: String,
    backing_indices: Vec<String>,
    timestamp_field: String,
    generation: u64,
}

impl From<DataStream> for PersistedDataStreamV1 {
    fn from(value: DataStream) -> Self {
        Self {
            name: value.name,
            backing_indices: value.backing_indices,
            timestamp_field: value.timestamp_field,
            generation: value.generation,
        }
    }
}

impl From<PersistedDataStreamV1> for DataStream {
    fn from(value: PersistedDataStreamV1) -> Self {
        Self {
            name: value.name,
            backing_indices: value.backing_indices,
            timestamp_field: value.timestamp_field,
            generation: value.generation,
        }
    }
}

/// Read and fully classify the cluster-state path once, before any index is
/// opened. The returned document is the only input later applied to live maps.
pub(crate) fn preflight(data_dir: &Path) -> PreparedClusterState {
    let path = data_dir.join("cluster_state.json");
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return PreparedClusterState {
                status: ClusterStateBootStatus::Absent,
                document: None,
            };
        }
        Err(err) => {
            return PreparedClusterState::blocked(
                &path,
                ClusterStateBlockKind::ReadFailure,
                format!("read failed: {err}"),
            );
        }
    };

    let value = match parse_unique_json(&bytes) {
        Ok(value) => value,
        Err(ParseFailure::Duplicate(detail)) => {
            return PreparedClusterState::blocked(
                &path,
                ClusterStateBlockKind::DuplicateKey,
                detail,
            );
        }
        Err(ParseFailure::Malformed(detail)) => {
            return PreparedClusterState::blocked(
                &path,
                ClusterStateBlockKind::MalformedJson,
                detail,
            );
        }
    };

    let object = match value.as_object() {
        Some(object) => object,
        None => {
            return PreparedClusterState::blocked(
                &path,
                ClusterStateBlockKind::IncompatibleSchema,
                "format-1 cluster state must be a JSON object",
            );
        }
    };

    let legacy = object.get("version");
    let future = object.get("format_version");
    match (legacy, future) {
        (Some(_), Some(_)) => {
            return PreparedClusterState::blocked(
                &path,
                ClusterStateBlockKind::AmbiguousDiscriminator,
                "both version and format_version are present",
            );
        }
        (None, Some(found)) => {
            return PreparedClusterState::blocked(
                &path,
                ClusterStateBlockKind::UnsupportedFormat,
                format!(
                    "unsupported cluster-state format: found format_version={}, supported version={CLUSTER_STATE_V1}",
                    discriminator_label(found)
                ),
            );
        }
        (None, None) => {
            return PreparedClusterState::blocked(
                &path,
                ClusterStateBlockKind::MissingDiscriminator,
                "required version discriminator is missing",
            );
        }
        (Some(found), None) => match found.as_u64() {
            Some(v) if v == u64::from(CLUSTER_STATE_V1) => {}
            Some(v) if v > u64::from(CLUSTER_STATE_V1) => {
                return PreparedClusterState::blocked(
                    &path,
                    ClusterStateBlockKind::UnsupportedFormat,
                    format!(
                        "unsupported cluster-state version: found={v}, supported={CLUSTER_STATE_V1}"
                    ),
                );
            }
            _ => {
                return PreparedClusterState::blocked(
                    &path,
                    ClusterStateBlockKind::InvalidDiscriminator,
                    format!(
                        "version must be the integer {CLUSTER_STATE_V1}, found {}",
                        discriminator_label(found)
                    ),
                );
            }
        },
    }

    match serde_json::from_value::<PersistedClusterStateV1>(value) {
        Ok(document) => {
            // Only a document that was completely classified as the exact
            // writable format 1 authorizes cleanup of the writer's fixed
            // staging name. A future/malformed document (or an absent live
            // path) may belong to another binary or an interrupted install;
            // a diagnostic-shell boot must not delete or create anything.
            let legacy_tmp = path.with_extension("tmp");
            if legacy_tmp.exists() {
                warn!("discarding an incomplete format-1 cluster-state staging file");
                let _ = std::fs::remove_file(legacy_tmp);
            }
            PreparedClusterState {
                status: ClusterStateBootStatus::LoadedV1,
                document: Some(document),
            }
        }
        Err(err) => PreparedClusterState::blocked(
            &path,
            ClusterStateBlockKind::IncompatibleSchema,
            format!("format-1 schema is not understood: {err}"),
        ),
    }
}

fn discriminator_label(value: &Value) -> String {
    match value {
        Value::String(value) => format!("string {value:?}"),
        other => other.to_string(),
    }
}

enum ParseFailure {
    Duplicate(String),
    Malformed(String),
}

fn parse_unique_json(bytes: &[u8]) -> Result<Value, ParseFailure> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = UniqueValue::deserialize(&mut deserializer).map_err(classify_parse_error)?;
    deserializer.end().map_err(classify_parse_error)?;
    Ok(value.0)
}

fn classify_parse_error(err: serde_json::Error) -> ParseFailure {
    let detail = err.to_string();
    if detail.contains(DUPLICATE_KEY_MARKER) {
        ParseFailure::Duplicate(detail)
    } else {
        ParseFailure::Malformed(detail)
    }
}

struct UniqueValue(Value);

impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(UniqueValueVisitor)
    }
}

struct UniqueValueVisitor;

impl<'de> Visitor<'de> for UniqueValueVisitor {
    type Value = UniqueValue;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("one JSON value without duplicate object keys")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Number(value.into())))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Number(value.into())))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Number::from_f64(value)
            .map(Value::Number)
            .map(UniqueValue)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::String(value.to_owned())))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::String(value)))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Null))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Null))
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        UniqueValue::deserialize(deserializer)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element::<UniqueValue>()? {
            values.push(value.0);
        }
        Ok(UniqueValue(Value::Array(values)))
    }

    fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut seen = HashSet::new();
        let mut values = Map::new();
        while let Some(key) = object.next_key::<String>()? {
            if !seen.insert(key.clone()) {
                return Err(A::Error::custom(format!("{DUPLICATE_KEY_MARKER} {key:?}")));
            }
            let value = object.next_value::<UniqueValue>()?;
            values.insert(key, value.0);
        }
        Ok(UniqueValue(Value::Object(values)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Default)]
    struct LogBuffer(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    struct LogWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for LogWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
        type Writer = LogWriter;

        fn make_writer(&'a self) -> Self::Writer {
            LogWriter(self.0.clone())
        }
    }

    fn capture_preflight_log(dir: &Path) -> (PreparedClusterState, String) {
        let buffer = LogBuffer::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(buffer.clone())
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);
        let prepared = preflight(dir);
        drop(guard);
        let bytes = buffer.0.lock().unwrap().clone();
        (prepared, String::from_utf8(bytes).unwrap())
    }

    fn exact_v1(extra: &str) -> Vec<u8> {
        format!(
            r#"{{
                "version": 1,
                "index_templates": {{}},
                "legacy_templates": {{}},
                "component_templates": {{}},
                "pipelines": {{}},
                "data_streams": {{}},
                "ilm_policies": {{{extra}}}
            }}"#
        )
        .into_bytes()
    }

    fn preflight_bytes(bytes: &[u8]) -> PreparedClusterState {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("cluster_state.json"), bytes).unwrap();
        preflight(dir.path())
    }

    #[test]
    fn cluster_state_format1_parser_accepts_exact_v1_and_open_values() {
        let prepared = preflight_bytes(&exact_v1(
            r#""policy": {"arbitrary": {"future-user-key": [1, true, null]}}"#,
        ));
        assert_eq!(prepared.status, ClusterStateBootStatus::LoadedV1);
        assert!(prepared.document.is_some());
    }

    #[test]
    fn cluster_state_format1_parser_blocks_every_invalid_discriminator_shape() {
        let cases: &[(&[u8], ClusterStateBlockKind)] = &[
            (br#"{}"#, ClusterStateBlockKind::MissingDiscriminator),
            (
                br#"{"version":0}"#,
                ClusterStateBlockKind::InvalidDiscriminator,
            ),
            (
                br#"{"version":-1}"#,
                ClusterStateBlockKind::InvalidDiscriminator,
            ),
            (
                br#"{"version":1.0}"#,
                ClusterStateBlockKind::InvalidDiscriminator,
            ),
            (
                br#"{"version":"1"}"#,
                ClusterStateBlockKind::InvalidDiscriminator,
            ),
            (
                br#"{"version":null}"#,
                ClusterStateBlockKind::InvalidDiscriminator,
            ),
            (
                br#"{"version":4294967296}"#,
                ClusterStateBlockKind::UnsupportedFormat,
            ),
            (
                br#"{"version":2}"#,
                ClusterStateBlockKind::UnsupportedFormat,
            ),
            (
                br#"{"format_version":2}"#,
                ClusterStateBlockKind::UnsupportedFormat,
            ),
            (
                br#"{"version":1,"format_version":2}"#,
                ClusterStateBlockKind::AmbiguousDiscriminator,
            ),
        ];
        for (bytes, expected) in cases {
            let prepared = preflight_bytes(bytes);
            assert_eq!(prepared.status.block_kind(), Some(expected), "{bytes:?}");
        }
    }

    #[test]
    fn cluster_state_format1_parser_reports_exact_found_and_supported_versions() {
        let prepared = preflight_bytes(br#"{"format_version":2}"#);
        assert_eq!(
            prepared.status,
            ClusterStateBootStatus::Blocked {
                kind: ClusterStateBlockKind::UnsupportedFormat,
                detail:
                    "unsupported cluster-state format: found format_version=2, supported version=1"
                        .to_owned(),
            }
        );

        let prepared = preflight_bytes(br#"{"version":2}"#);
        assert_eq!(
            prepared.status,
            ClusterStateBootStatus::Blocked {
                kind: ClusterStateBlockKind::UnsupportedFormat,
                detail: "unsupported cluster-state version: found=2, supported=1".to_owned(),
            }
        );
    }

    #[test]
    fn cluster_state_format1_parser_rejects_trailing_non_whitespace() {
        let mut bytes = exact_v1("");
        bytes.extend_from_slice(b" true");
        let prepared = preflight_bytes(&bytes);
        assert_eq!(
            prepared.status.block_kind(),
            Some(&ClusterStateBlockKind::MalformedJson)
        );
    }

    #[test]
    fn every_representative_block_class_logs_kind_detail_and_operator_path_at_boot() {
        let cases: &[(&[u8], ClusterStateBlockKind, &str)] = &[
            (
                br#"{"format_version":2}"#,
                ClusterStateBlockKind::UnsupportedFormat,
                "found format_version=2, supported version=1",
            ),
            (
                br#"{"version":1}"#,
                ClusterStateBlockKind::IncompatibleSchema,
                "format-1 schema is not understood",
            ),
            (
                br#"{"version":1,"version":1}"#,
                ClusterStateBlockKind::DuplicateKey,
                "duplicate JSON object key",
            ),
        ];
        for (bytes, kind, detail) in cases {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("cluster_state.json");
            std::fs::write(&path, bytes).unwrap();
            let (prepared, log) = capture_preflight_log(dir.path());
            assert_eq!(prepared.status.block_kind(), Some(kind));
            assert!(
                log.contains("cluster state was rejected at boot"),
                "{kind:?}: {log}"
            );
            assert!(
                log.contains(
                    "start a compatible XERJ build or restore supported bytes, then restart"
                ),
                "{kind:?}: {log}"
            );
            assert!(log.contains(&format!("kind={kind:?}")), "{kind:?}: {log}");
            assert!(log.contains(detail), "{kind:?}: {log}");
            assert!(log.contains(&path.display().to_string()), "{kind:?}: {log}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn read_failure_logs_the_same_boot_diagnostic_contract() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cluster_state.json");
        symlink("cluster_state.json", &path).unwrap();
        let (prepared, log) = capture_preflight_log(dir.path());
        assert_eq!(
            prepared.status.block_kind(),
            Some(&ClusterStateBlockKind::ReadFailure)
        );
        assert!(log.contains("kind=ReadFailure"), "{log}");
        assert!(log.contains("read failed:"), "{log}");
        assert!(log.contains(&path.display().to_string()), "{log}");
    }

    #[test]
    fn cluster_state_format1_parser_rejects_duplicates_at_every_depth() {
        let top = exact_v1(r#""a": {}, "a": {}"#);
        let nested = exact_v1(r#""a": {"policy": {"x": 1, "x": 2}}"#);
        for bytes in [&top, &nested] {
            let prepared = preflight_bytes(bytes);
            assert_eq!(
                prepared.status.block_kind(),
                Some(&ClusterStateBlockKind::DuplicateKey)
            );
        }
    }

    #[test]
    fn cluster_state_format1_parser_rejects_unknown_and_missing_typed_fields() {
        let unknown_envelope = {
            let mut bytes = exact_v1("");
            let end = bytes.iter().rposition(|byte| *byte == b'}').unwrap();
            bytes.splice(end..end, b",\"future\":true".iter().copied());
            bytes
        };
        let unknown_template = br#"{
            "version":1,
            "index_templates":{"x":{"index_patterns":[],"settings":{},"mappings":{},"priority":0,"future":true}},
            "legacy_templates":{},"component_templates":{},"pipelines":{},"data_streams":{},"ilm_policies":{}
        }"#;
        let missing_map = br#"{
            "version":1,"index_templates":{},"legacy_templates":{},
            "component_templates":{},"pipelines":{},"data_streams":{}
        }"#;
        for bytes in [unknown_envelope.as_slice(), unknown_template, missing_map] {
            let prepared = preflight_bytes(bytes);
            assert_eq!(
                prepared.status.block_kind(),
                Some(&ClusterStateBlockKind::IncompatibleSchema)
            );
        }
    }

    #[test]
    fn blocked_or_absent_preflight_neither_cleans_staging_nor_writes_salvage() {
        let cases: &[Option<&[u8]>] = &[
            Some(br#"{"format_version":2}"#),
            Some(br#"{"version":1,"cut":true"#),
            None,
        ];
        for live in cases {
            let dir = tempfile::tempdir().unwrap();
            let state = dir.path().join("cluster_state.json");
            if let Some(bytes) = live {
                std::fs::write(&state, bytes).unwrap();
            }
            let staging = dir.path().join("cluster_state.tmp");
            let staging_bytes = b"owned-by-another-or-interrupted-writer";
            std::fs::write(&staging, staging_bytes).unwrap();

            let prepared = preflight(dir.path());
            assert!(
                !prepared.status.is_writable() || live.is_none(),
                "fixture must not classify as loaded format 1"
            );
            assert_eq!(std::fs::read(&staging).unwrap(), staging_bytes);
            assert!(!dir.path().join("cluster_state.corrupt.json").exists());
        }
    }

    #[test]
    fn loaded_format1_alone_authorizes_legacy_staging_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("cluster_state.json"), exact_v1("")).unwrap();
        let staging = dir.path().join("cluster_state.tmp");
        std::fs::write(&staging, b"interrupted format-1 stage").unwrap();

        let prepared = preflight(dir.path());
        assert_eq!(prepared.status, ClusterStateBootStatus::LoadedV1);
        assert!(!staging.exists());
    }
}
