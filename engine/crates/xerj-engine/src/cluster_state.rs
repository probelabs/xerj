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

/// Serialises every test in this crate's `--lib` binary that can reach the
/// `error!` in `PreparedClusterState::blocked` above.
///
/// `tracing-core` keeps one process-global `Interest` per callsite, and while a
/// single dispatcher is registered it recomputes that slot from the *calling
/// thread's* default subscriber — `rebuild_callsite_interest` ->
/// `Rebuilder::JustOne` -> `dispatcher::get_default`, tracing-core-0.1.36
/// `src/callsite.rs:490` and `:564`-`:565`.  A thread with no subscriber
/// resolves to `NoSubscriber`, whose `register_callsite` returns
/// `Interest::never()` (`src/subscriber.rs:676`).  So a test that reaches
/// `blocked` with no subscriber installed pins the boot diagnostic to "never"
/// for the whole process if it is the thread that first registers that
/// callsite — and a `capture_preflight_log` running concurrently comes back
/// with an empty string.  That is the `cluster_state` half of issue #372:
/// `every_representative_block_class_logs_kind_detail_and_operator_path_at_boot`
/// failed with an empty log, and only at full test parallelism, because
/// `--test-threads=2` rarely produces the losing interleaving.
///
/// A poison that lands *before* a capture starts is harmless — installing a
/// subscriber is `Dispatch::new`, which unconditionally rebuilds every
/// registered callsite against the live dispatchers (`src/dispatcher.rs:479`
/// -> `src/callsite.rs:484`), and our subscriber is one of them.  Only a poison
/// that lands *inside* the install-subscriber-then-log window can blank the
/// capture, so mutual exclusion is the whole fix.
///
/// Scope, exactly.  This is `pub(crate)` rather than private to
/// `cluster_state::tests` because `blocked` is reachable from a *second* test
/// module in the same binary, so "no interleaving remains" is only true if that
/// one takes the lock too.  `blocked` has exactly one production caller —
/// `preflight`, below — and `preflight` has exactly one production caller,
/// `Engine::new` (engine.rs:763), so the reachers are enumerable by grep.  All
/// of them, as of this commit:
///
///   - `tests::capture_preflight_log` — holds it across
///     install-subscriber-then-log.  The victim.
///   - `tests::preflight_serialised`, and `tests::preflight_bytes` through it —
///     no subscriber installed, so a poisoner.
///   - `tests::poison_boot_diagnostic_callsite` — the regression test's
///     deliberate model of a subscriber-less sibling.
///   - `lifecycle::tests::cluster_state_format1_lifecycle_delete_and_detach_guard_before_side_effects`
///     — writes a `format_version: 2` state and calls `Engine::new`, which
///     reaches `preflight`, classifies `UnsupportedFormat` and lands here.
///     Also subscriber-less, so also a poisoner.
///
/// That last one is latent rather than live, and the reason is test-name
/// ordering, not design: libtest runs a byte-sorted list, in which
/// `cluster_state::tests::*` ends at #106 of 601 and the `lifecycle` case is
/// #436 (measured with `--list` at this commit), and a callsite's registration
/// is one-shot per process — the first capture wins it and nothing `lifecycle`
/// does afterwards re-registers it.  Nothing *enforces* that ordering, so the
/// lock is taken there rather than argued away.  Any new test that boots an
/// `Engine` over a cluster state which classifies as blocked owes the same
/// call.
#[cfg(test)]
pub(crate) static PREFLIGHT_CALLSITE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Lock `PREFLIGHT_CALLSITE`, ignoring poisoning.
///
/// A test that panics while holding it has already failed; propagating the
/// poison would convert one real failure into a cascade of unrelated ones.
#[cfg(test)]
pub(crate) fn lock_preflight_callsite() -> std::sync::MutexGuard<'static, ()> {
    PREFLIGHT_CALLSITE.lock().unwrap_or_else(|e| e.into_inner())
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
        let _serialised = lock_preflight_callsite();
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
        preflight_serialised(dir.path())
    }

    /// `preflight`, but not while a `capture_preflight_log` is in flight.
    ///
    /// See `PREFLIGHT_CALLSITE`: this call has no subscriber installed, so it is
    /// the poisoner, not the victim.
    fn preflight_serialised(dir: &Path) -> PreparedClusterState {
        let _serialised = lock_preflight_callsite();
        preflight(dir)
    }

    /// Reproduce, on demand, what a subscriber-less sibling does to the
    /// process-global callsite interest cache.
    ///
    /// `preflight_bytes` only pins the boot diagnostic to `Interest::never()` on
    /// the process's *first* registration of that callsite, and a test cannot
    /// re-trigger a one-shot registration.  Calling `rebuild_interest_cache`
    /// from a thread with no subscriber reaches the identical end state: the
    /// rebuild resolves to `NoSubscriber`, whose `register_callsite` is
    /// `Interest::never()`.  Serialised on `PREFLIGHT_CALLSITE` because that is
    /// what makes it a faithful model of a sibling rather than an unfair
    /// hammer — a sibling holds the lock too.
    fn poison_boot_diagnostic_callsite() {
        let _serialised = lock_preflight_callsite();
        tracing::callsite::rebuild_interest_cache();
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

    /// Regression for the `cluster_state` half of #372.
    ///
    /// The reported failure was this module's boot-diagnostic assertions firing
    /// with an *empty* captured log at full test parallelism.  The cause is a
    /// process-global cache, not the subscriber: a sibling test touching the
    /// same callsite from a thread with no subscriber pins it to
    /// `Interest::never()`, and if that lands inside a capture's window the
    /// event never reaches the writer.  Model that sibling on a second thread
    /// and assert every capture still sees the diagnostic.
    #[test]
    fn a_concurrent_subscriber_less_preflight_cannot_blank_the_boot_diagnostic_capture() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cluster_state.json");
        std::fs::write(&path, br#"{"format_version":2}"#).unwrap();

        let stop = Arc::new(AtomicBool::new(false));
        let poisoner = {
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    poison_boot_diagnostic_callsite();
                    std::thread::yield_now();
                }
            })
        };

        let mut blank = 0usize;
        for _ in 0..200 {
            let (prepared, log) = capture_preflight_log(dir.path());
            assert_eq!(
                prepared.status.block_kind(),
                Some(&ClusterStateBlockKind::UnsupportedFormat)
            );
            if !log.contains("cluster state was rejected at boot") {
                blank += 1;
            }
        }
        stop.store(true, Ordering::Relaxed);
        poisoner.join().unwrap();

        assert_eq!(
            blank, 0,
            "a concurrent subscriber-less preflight blanked {blank}/200 boot-diagnostic captures"
        );
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

            let prepared = preflight_serialised(dir.path());
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

        let prepared = preflight_serialised(dir.path());
        assert_eq!(prepared.status, ClusterStateBootStatus::LoadedV1);
        assert!(!staging.exists());
    }
}
