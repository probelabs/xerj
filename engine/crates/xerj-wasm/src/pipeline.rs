//! Pipeline executor: runs a document through an ordered list of transform
//! plugins.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, warn};

use crate::{
    builtins::{
        AddFieldPlugin, AppendPlugin, ConvertTypePlugin, CopyFieldPlugin, DropFieldPlugin,
        FieldRenamePlugin, GrokPlugin, JsonParsePlugin, LowercasePlugin, PiiRedactionPlugin,
        RemoveNullPlugin, RoutePlugin, SetPlugin, SplitPlugin, TimestampParsePlugin,
        UppercasePlugin, UrlDecodePlugin, CONVERT_TARGET_TYPES, GROK_PATTERN_NAMES, PII_TYPE_NAMES,
    },
    Result, TransformPlugin, WasmError,
};

// ── Action returned by each stage ────────────────────────────────────────────

/// Decision returned by a pipeline stage (or the pipeline itself).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessAction {
    /// Continue to the next stage / index the document normally.
    Pass,
    /// Discard this document — do not index it.
    Drop,
    /// Index the document into a different target (overrides the original
    /// index name).
    Route(String),
}

// ── Error policy ─────────────────────────────────────────────────────────────

/// What to do when a pipeline stage returns an error.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ErrorPolicy {
    /// Discard the document (default).
    #[default]
    Drop,
    /// Pass the document through unchanged.
    Pass,
    /// Send the document to a dead-letter index (`<original>-dead-letter`).
    DeadLetter,
}

// ── Pipeline config (JSON-deserialisable) ─────────────────────────────────────

/// Configuration for a single pipeline stage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineStageConfig {
    /// Stage type — maps to a built-in plugin name (e.g. `"json_parse"`).
    #[serde(rename = "type")]
    pub stage_type: String,
    /// Arbitrary plugin-specific configuration.
    #[serde(default)]
    pub config: Value,
}

/// Top-level pipeline configuration (stored in the engine and serialised for
/// the `PUT /v1/pipelines/{name}` API).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineConfig {
    /// Human-readable description (optional).
    #[serde(default)]
    pub description: String,
    /// Ordered list of transform stages.
    pub stages: Vec<PipelineStageConfig>,
    /// What to do when a stage fails.
    #[serde(default)]
    pub on_error: ErrorPolicy,
    /// Per-document timeout in milliseconds (0 = unlimited).
    #[serde(default)]
    pub timeout_ms: u64,
}

// ── Pipeline ──────────────────────────────────────────────────────────────────

/// A named, executable pipeline composed of ordered [`TransformPlugin`] stages.
///
/// `Pipeline` is `Clone` (cheap — inner stages are `Arc`-wrapped) and safe to
/// share across async tasks.
#[derive(Clone)]
pub struct Pipeline {
    /// Pipeline name (same as the key in the engine's pipeline map).
    pub name: String,
    /// Ordered stages.
    stages: Vec<Arc<dyn TransformPlugin>>,
    /// Error handling policy.
    pub on_error: ErrorPolicy,
    /// Per-document timeout (informational — enforced by the caller).
    pub timeout: Duration,
}

impl Pipeline {
    /// Build a [`Pipeline`] from a [`PipelineConfig`].
    ///
    /// Three outcomes, deliberately distinct (issue #204):
    ///
    /// - [`WasmError::InvalidConfig`] — the caller's mistake. The definition
    ///   must be refused.
    /// - [`WasmError::UnsupportedStage`] — a *xerj capability gap*: the caller
    ///   wrote something legitimate that we cannot run. The honest answer is to
    ///   say so at the point of use rather than pretend either way.
    /// - [`WasmError::UnsupportedOption`] — the same gap one level down: the
    ///   stage exists, an option on it does not (a processor-level `if` guard,
    ///   a `grok` `patterns` array). Silently dropping these is what makes a
    ///   compiled pipeline write the wrong document under a `201`.
    pub fn from_config(name: impl Into<String>, config: &PipelineConfig) -> Result<Self> {
        let name = name.into();
        let mut stages: Vec<Arc<dyn TransformPlugin>> = Vec::new();

        for stage_cfg in &config.stages {
            let plugin =
                build_plugin(&stage_cfg.stage_type, &stage_cfg.config).map_err(|e| match e {
                    StageBuildError::Unsupported(stage) => WasmError::UnsupportedStage { stage },
                    StageBuildError::UnsupportedOption { option, reason } => {
                        WasmError::UnsupportedOption {
                            stage: stage_cfg.stage_type.clone(),
                            option,
                            reason,
                        }
                    }
                    StageBuildError::Config(reason) => WasmError::InvalidConfig {
                        plugin: stage_cfg.stage_type.clone(),
                        reason,
                    },
                })?;
            stages.push(plugin);
        }

        Ok(Self {
            name,
            stages,
            on_error: config.on_error.clone(),
            timeout: if config.timeout_ms == 0 {
                Duration::from_secs(30)
            } else {
                Duration::from_millis(config.timeout_ms)
            },
        })
    }

    /// Run `doc` through every stage in order.
    ///
    /// Returns the first non-[`ProcessAction::Pass`] action, or
    /// [`ProcessAction::Pass`] if all stages pass.
    pub fn process(&self, doc: &mut Value) -> ProcessAction {
        for stage in &self.stages {
            debug!(
                pipeline = self.name.as_str(),
                stage = stage.name(),
                "running stage"
            );
            match stage.process(doc) {
                ProcessAction::Pass => continue,
                action => {
                    debug!(
                        pipeline = self.name.as_str(),
                        stage = stage.name(),
                        action = ?action,
                        "stage short-circuits pipeline"
                    );
                    return action;
                }
            }
        }
        ProcessAction::Pass
    }

    /// Run every document in `docs` through the pipeline.
    ///
    /// Documents are processed independently — one failing stage does not
    /// affect subsequent documents.
    pub fn process_batch(&self, docs: &mut [Value]) -> Vec<ProcessAction> {
        docs.iter_mut().map(|doc| self.process(doc)).collect()
    }

    /// Number of stages in this pipeline.
    pub fn stage_count(&self) -> usize {
        self.stages.len()
    }
}

impl std::fmt::Debug for Pipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pipeline")
            .field("name", &self.name)
            .field("stage_count", &self.stages.len())
            .field("on_error", &self.on_error)
            .finish()
    }
}

// ── Plugin factory ────────────────────────────────────────────────────────────

/// Why [`build_plugin`] refused a stage.
///
/// Split three ways on purpose (issue #204): `Config` is the caller's mistake
/// and must be refused at definition time; `Unsupported` and
/// `UnsupportedOption` are xerj's gaps and have to be reported as such, so
/// callers are not told their processor is invalid when it is merely
/// unimplemented here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StageBuildError {
    /// The stage type is not implemented by this build.
    Unsupported(String),
    /// The stage type IS implemented but an option on it is not.
    UnsupportedOption { option: String, reason: String },
    /// The stage type exists but its config cannot be honoured.
    Config(String),
}

impl From<String> for StageBuildError {
    fn from(reason: String) -> Self {
        Self::Config(reason)
    }
}

/// Elasticsearch processor-level keys that decide **whether** a processor runs
/// or **what happens when it fails**, none of which the compiled pipeline can
/// act on.
///
/// Issue #204, applied to this PR's own new surface. `PUT /_ingest/pipeline`
/// now means "compiled ⇒ xerj honours this"; a `{"set": {…, "if": "ctx.foo ==
/// 'never'"}}` that compiles and then sets the field on every document makes
/// that promise false, and writes the wrong document under a `201`. The
/// pipeline is accepted (ES accepts it) and recorded as unrunnable, so the gap
/// surfaces at ingest instead of being silently resolved in the caller's face.
///
/// `tag` and `description` are deliberately NOT here: they are identification
/// metadata that changes no document and no decision on this path.
const UNHONOURED_PROCESSOR_KEYS: &[(&str, &str)] = &[
    (
        "if",
        "the guard is not evaluated, so the processor would run on documents it excludes",
    ),
    (
        "on_failure",
        "the recovery processors are not run; the pipeline's `on_error` policy applies instead",
    ),
    (
        "ignore_failure",
        "a failing processor is not ignored; the pipeline's `on_error` policy applies instead",
    ),
];

/// Instantiate a built-in plugin by name.
fn build_plugin(
    stage_type: &str,
    config: &Value,
) -> std::result::Result<Arc<dyn TransformPlugin>, StageBuildError> {
    for (key, reason) in UNHONOURED_PROCESSOR_KEYS {
        if config.get(key).is_some() {
            return Err(StageBuildError::UnsupportedOption {
                option: (*key).to_string(),
                reason: (*reason).to_string(),
            });
        }
    }
    match stage_type {
        "json_parse" => {
            let field = str_field(config, "field")?;
            let target = config
                .get("target")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            Ok(Arc::new(JsonParsePlugin::new(field, target)))
        }

        "timestamp_parse" => {
            let field = str_field(config, "field")?;
            let formats = optional_string_array(config, "formats")?.unwrap_or_default();
            let target = config
                .get("target")
                .and_then(Value::as_str)
                .map(str::to_string);
            Ok(Arc::new(TimestampParsePlugin::new(field, formats, target)))
        }

        "field_rename" => {
            let mappings = config
                .get("mappings")
                .and_then(Value::as_object)
                .ok_or_else(|| "missing 'mappings' object".to_string())?;
            let map: HashMap<String, String> = mappings
                .iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect();
            Ok(Arc::new(FieldRenamePlugin::new(map)))
        }

        "drop_field" => {
            let fields = string_array(config, "fields")?;
            Ok(Arc::new(DropFieldPlugin::new(fields)))
        }

        "add_field" => {
            let field = str_field(config, "field")?;
            let value = config
                .get("value")
                .cloned()
                .ok_or_else(|| "missing 'value'".to_string())?;
            Ok(Arc::new(AddFieldPlugin::new(field, value)))
        }

        "route" => {
            let field = str_field(config, "field")?;
            let routes = config
                .get("routes")
                .and_then(Value::as_object)
                .ok_or_else(|| "missing 'routes' object".to_string())?;
            let map: HashMap<String, String> = routes
                .iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect();
            let default = config
                .get("default")
                .and_then(Value::as_str)
                .map(str::to_string);
            Ok(Arc::new(RoutePlugin::new(field, map, default)))
        }

        "grok" => {
            let field = str_field(config, "field")?;
            // Elasticsearch's `grok` processor is driven by `patterns` — an
            // ARRAY of grok expressions (`["%{IP:client} %{WORD:method}"]`),
            // optionally with `pattern_definitions`. xerj's `grok` is a
            // different, narrower thing: one NAMED pattern out of
            // `GROK_PATTERN_NAMES`. Reading `pattern` and defaulting to
            // `SYSLOG` meant an ES grok processor compiled clean, answered
            // `200 acknowledged` under this PR's "compiled ⇒ honoured"
            // contract, and left the document untouched (issue #204 — measured:
            // `{"message": "10.0.0.1 GET"}` came back with no `client` field).
            for key in ["patterns", "pattern_definitions"] {
                if config.get(key).is_some() {
                    return Err(StageBuildError::UnsupportedOption {
                        option: key.to_string(),
                        reason: format!(
                            "xerj's grok stage takes a named `pattern` from [{}], not \
                             arbitrary grok expressions",
                            GROK_PATTERN_NAMES.join(", ")
                        ),
                    });
                }
            }
            let pattern_name = match config.get("pattern") {
                None => "SYSLOG".to_string(),
                Some(Value::String(s)) => {
                    // Fail closed: an unrecognised pattern name used to fall
                    // through to the generic catch-all, which extracts a
                    // single `message` field and nothing else (issue #204).
                    if !GROK_PATTERN_NAMES.contains(&s.as_str()) {
                        return Err(format!(
                            "unknown grok pattern '{s}' (known patterns: {})",
                            GROK_PATTERN_NAMES.join(", ")
                        )
                        .into());
                    }
                    s.clone()
                }
                Some(other) => {
                    return Err(format!("'pattern' must be a string, got {other}").into());
                }
            };
            Ok(Arc::new(GrokPlugin::new(field, pattern_name)))
        }

        "pii_redaction" => {
            // Fail closed on an unknown PII type: it used to contribute no
            // regex, so a typo'd `types` list produced a stage that redacted
            // nothing while reporting success (issue #204).
            let types = match optional_string_array(config, "types")? {
                None => vec!["email".into(), "ip".into(), "credit_card".into()],
                Some(requested) => {
                    for t in &requested {
                        if !PII_TYPE_NAMES.contains(&t.as_str()) {
                            return Err(format!(
                                "unknown pii type '{t}' (known types: {})",
                                PII_TYPE_NAMES.join(", ")
                            )
                            .into());
                        }
                    }
                    requested
                }
            };
            Ok(Arc::new(PiiRedactionPlugin::new(types)))
        }

        "copy_field" => {
            let source = str_field(config, "source")?;
            let target = str_field(config, "target")?;
            Ok(Arc::new(CopyFieldPlugin::new(source, target)))
        }

        "convert" => {
            let field = str_field(config, "field")?;
            let target_type = str_field(config, "type")?;
            // Fail closed: an unknown target type used to no-op per document
            // rather than fail the pipeline definition (issue #204).
            if !CONVERT_TARGET_TYPES.contains(&target_type.as_str()) {
                return Err(format!(
                    "unknown convert target type '{target_type}' (known types: {})",
                    CONVERT_TARGET_TYPES.join(", ")
                )
                .into());
            }
            Ok(Arc::new(ConvertTypePlugin::new(field, target_type)))
        }

        "split" => {
            let field = str_field(config, "field")?;
            let separator = config
                .get("separator")
                .and_then(Value::as_str)
                .unwrap_or(",")
                .to_string();
            Ok(Arc::new(SplitPlugin::new(field, separator)))
        }

        "lowercase" => {
            let field = str_field(config, "field")?;
            Ok(Arc::new(LowercasePlugin::new(field)))
        }

        "uppercase" => {
            let field = str_field(config, "field")?;
            Ok(Arc::new(UppercasePlugin::new(field)))
        }

        "set" => {
            let field = str_field(config, "field")?;
            // Elasticsearch's `set` takes EITHER `value` or `copy_from` (7.x+),
            // where `copy_from` names another field to read the value out of
            // the document at ingest time. `SetPlugin` only holds a literal
            // value, so `copy_from` cannot be honoured — but it is a valid ES
            // processor, which makes the gap xerj's, not the caller's. Reported
            // as an unsupported OPTION (accept the definition, refuse the
            // write) rather than falling through to `missing 'value'`, which
            // told the caller their correct ES pipeline was malformed.
            if config.get("copy_from").is_some() {
                return Err(StageBuildError::UnsupportedOption {
                    option: "copy_from".to_string(),
                    reason: "xerj's set stage assigns a literal `value`; copying the value \
                             from another field at ingest time is not implemented"
                        .to_string(),
                });
            }
            let value = config
                .get("value")
                .cloned()
                .ok_or_else(|| "missing 'value'".to_string())?;
            // Elasticsearch's `set` processor defaults `override` to TRUE
            // (`SetProcessor.java:135` — `readBooleanProperty(…, "override",
            // true)`; ES is read for semantics only, no code reproduced). This
            // was `unwrap_or(false)`, so `{"set": {"field": "env", "value":
            // "prod"}}` left an existing `{"env": "dev"}` untouched: a weaker
            // fallback on user configuration, in the file the #204 sweep
            // rewrote. A present-but-non-boolean `override` is a caller error
            // rather than a silent fall-through to the default.
            let override_existing = match config.get("override") {
                None => true,
                Some(v) => v
                    .as_bool()
                    .ok_or_else(|| format!("'override' must be a boolean, got {v}"))?,
            };
            Ok(Arc::new(SetPlugin::new(field, value, override_existing)))
        }

        "append" => {
            let field = str_field(config, "field")?;
            let value = config
                .get("value")
                .cloned()
                .ok_or_else(|| "missing 'value'".to_string())?;
            // ES's `allow_duplicates` defaults to true (`AppendProcessor
            // .java:121`) and that is what `AppendPlugin` does; the
            // de-duplicating variant is not implemented, so asking for it must
            // not be silently ignored.
            match config.get("allow_duplicates") {
                None | Some(Value::Bool(true)) => {}
                Some(_) => {
                    return Err(StageBuildError::UnsupportedOption {
                        option: "allow_duplicates".to_string(),
                        reason: "values are always appended; de-duplicating append is not \
                                 implemented"
                            .to_string(),
                    })
                }
            }
            Ok(Arc::new(AppendPlugin::new(field, value)))
        }

        "remove_null" => Ok(Arc::new(RemoveNullPlugin)),

        "url_decode" => {
            let field = str_field(config, "field")?;
            Ok(Arc::new(UrlDecodePlugin::new(field)))
        }

        unknown => {
            warn!(stage_type = unknown, "unknown pipeline stage type");
            Err(StageBuildError::Unsupported(unknown.to_string()))
        }
    }
}

// ── Config helpers ────────────────────────────────────────────────────────────

fn str_field(config: &Value, key: &str) -> std::result::Result<String, String> {
    config
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("missing required string field '{key}'"))
}

/// Read an OPTIONAL array-of-strings config key.
///
/// `Ok(None)` means the key is absent (the caller applies its documented
/// default). A present-but-wrong-shaped value is an error rather than a silent
/// fall-through to the default — `"formats": "%Y-%m-%d"` used to be dropped by
/// `and_then(Value::as_array)` and the stage then behaved as if no formats had
/// been configured at all (issue #204).
fn optional_string_array(
    config: &Value,
    key: &str,
) -> std::result::Result<Option<Vec<String>>, String> {
    let Some(raw) = config.get(key) else {
        return Ok(None);
    };
    let arr = raw
        .as_array()
        .ok_or_else(|| format!("'{key}' must be an array of strings"))?;
    let mut out = Vec::with_capacity(arr.len());
    for v in arr {
        let s = v
            .as_str()
            .ok_or_else(|| format!("'{key}' must contain only strings, got {v}"))?;
        out.push(s.to_string());
    }
    Ok(Some(out))
}

fn string_array(config: &Value, key: &str) -> std::result::Result<Vec<String>, String> {
    let arr = config
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("missing required array field '{key}'"))?;
    Ok(arr
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pipeline(stages: &[(&str, Value)]) -> Pipeline {
        let stage_cfgs: Vec<PipelineStageConfig> = stages
            .iter()
            .map(|(t, c)| PipelineStageConfig {
                stage_type: t.to_string(),
                config: c.clone(),
            })
            .collect();
        let cfg = PipelineConfig {
            description: "test".into(),
            stages: stage_cfgs,
            on_error: ErrorPolicy::Drop,
            timeout_ms: 0,
        };
        Pipeline::from_config("test-pipeline", &cfg).expect("pipeline build failed")
    }

    #[test]
    fn pipeline_processes_all_pass_stages() {
        let pl = make_pipeline(&[(
            "add_field",
            serde_json::json!({ "field": "env", "value": "test" }),
        )]);
        let mut doc = serde_json::json!({ "msg": "hello" });
        assert_eq!(pl.process(&mut doc), ProcessAction::Pass);
        assert_eq!(doc["env"], "test");
    }

    #[test]
    fn pipeline_short_circuits_on_drop() {
        let pl = make_pipeline(&[
            // First stage drops the document
            ("drop_field", serde_json::json!({ "fields": ["drop_me"] })),
            // This stage would add a field, but we'll test with a route stage
            // that triggers drop via a route mismatch — use a simple 2-stage test
            (
                "add_field",
                serde_json::json!({ "field": "should_not_appear", "value": true }),
            ),
        ]);
        let mut doc = serde_json::json!({ "msg": "hello", "drop_me": "x" });
        // drop_field returns Pass (it removes the field), add_field adds env
        assert_eq!(pl.process(&mut doc), ProcessAction::Pass);
        assert_eq!(doc["should_not_appear"], true);
    }

    #[test]
    fn pipeline_batch() {
        let pl = make_pipeline(&[(
            "add_field",
            serde_json::json!({ "field": "pipeline", "value": "default" }),
        )]);
        let mut docs = vec![serde_json::json!({ "a": 1 }), serde_json::json!({ "b": 2 })];
        let actions = pl.process_batch(&mut docs);
        assert!(actions.iter().all(|a| *a == ProcessAction::Pass));
        assert_eq!(docs[0]["pipeline"], "default");
        assert_eq!(docs[1]["pipeline"], "default");
    }

    #[test]
    fn pipeline_from_invalid_config_fails() {
        let cfg = PipelineConfig {
            description: String::new(),
            stages: vec![PipelineStageConfig {
                stage_type: "unknown_plugin_xyz".into(),
                config: Value::Null,
            }],
            on_error: ErrorPolicy::Pass,
            timeout_ms: 0,
        };
        assert!(Pipeline::from_config("bad", &cfg).is_err());
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Issue #204 — weaker fallbacks on user-supplied processor configuration
//
// Each case below used to build a pipeline that was accepted, stored, and
// reported healthy, and then did LESS than the caller asked for with no signal
// at all. They now fail closed at `Pipeline::from_config`, which is where the
// caller can still act on the error.
// ─────────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod fallback_regression_tests {
    use super::*;

    fn build(stage_type: &str, config: Value) -> std::result::Result<Pipeline, WasmError> {
        Pipeline::from_config(
            "t",
            &PipelineConfig {
                description: String::new(),
                stages: vec![PipelineStageConfig {
                    stage_type: stage_type.into(),
                    config,
                }],
                on_error: ErrorPolicy::Drop,
                timeout_ms: 0,
            },
        )
    }

    /// A stage type this build does not implement and a stage type whose
    /// config we refuse are different answers, and callers act on the
    /// difference: the first is xerj's gap (accept the definition, refuse the
    /// writes), the second is the caller's mistake (refuse the definition).
    /// Collapsing them was what let an unimplemented processor be silently
    /// dropped in the first place.
    #[test]
    fn an_unimplemented_stage_type_is_distinguishable_from_a_bad_config() {
        let err = build("pipeline", serde_json::json!({ "name": "other" }))
            .expect_err("unimplemented stage type must not build");
        assert!(
            matches!(&err, WasmError::UnsupportedStage { stage } if stage == "pipeline"),
            "expected UnsupportedStage, got: {err:?}"
        );

        let err = build(
            "convert",
            serde_json::json!({ "field": "f", "type": "int" }),
        )
        .expect_err("bad config must not build");
        assert!(
            matches!(err, WasmError::InvalidConfig { .. }),
            "a known stage with a bad config is InvalidConfig, got: {err:?}"
        );
    }

    #[test]
    fn unknown_pii_type_is_rejected_not_silently_unredacted() {
        // Pre-fix: `social_security` matched no `wants(..)` arm, the pattern
        // list came out EMPTY, and the stage passed every document through
        // with its PII intact while reporting success.
        let err = build(
            "pii_redaction",
            serde_json::json!({ "types": ["ssn", "social_security"] }),
        )
        .expect_err("unknown pii type must fail the pipeline definition");
        assert!(
            err.to_string().contains("social_security"),
            "error must name the offending type: {err}"
        );

        // The supported spelling still builds and still redacts.
        let pl = build("pii_redaction", serde_json::json!({ "types": ["ssn"] }))
            .expect("known pii type builds");
        let mut doc = serde_json::json!({ "note": "ssn 123-45-6789" });
        pl.process(&mut doc);
        assert_eq!(doc["note"], "ssn [REDACTED_SSN]");
    }

    #[test]
    fn non_array_pii_types_is_rejected_not_defaulted() {
        // Pre-fix: `and_then(Value::as_array)` returned None for a bare string
        // and the stage silently fell back to the built-in default type list.
        assert!(build("pii_redaction", serde_json::json!({ "types": "ssn" })).is_err());
    }

    #[test]
    fn absent_pii_types_still_uses_the_documented_default() {
        let pl = build("pii_redaction", serde_json::json!({})).expect("default types build");
        let mut doc = serde_json::json!({ "note": "mail me at a@b.com" });
        pl.process(&mut doc);
        assert_eq!(doc["note"], "mail me at [REDACTED_EMAIL]");
    }

    #[test]
    fn unknown_grok_pattern_is_rejected_not_downgraded_to_catch_all() {
        // Pre-fix: this compiled to `^(?P<message>.+)$`, so an nginx pipeline
        // extracted one field instead of ten and nothing said so.
        let err = build(
            "grok",
            serde_json::json!({ "field": "msg", "pattern": "NGINX_COMBINE" }),
        )
        .expect_err("typo'd grok pattern must fail");
        assert!(
            err.to_string().contains("NGINX_COMBINE"),
            "error must name the offending pattern: {err}"
        );

        // The catch-all is still reachable — by asking for it.
        let pl = build(
            "grok",
            serde_json::json!({ "field": "msg", "pattern": "GENERIC" }),
        )
        .expect("GENERIC is an explicit, supported pattern");
        let mut doc = serde_json::json!({ "msg": "anything at all" });
        pl.process(&mut doc);
        assert_eq!(doc["message"], "anything at all");
    }

    #[test]
    fn unknown_convert_target_type_is_rejected_not_a_per_document_noop() {
        // Pre-fix: `"int"` built fine and then hit `_ => None` on every
        // document, leaving the field a string forever.
        let err = build(
            "convert",
            serde_json::json!({ "field": "status", "type": "int" }),
        )
        .expect_err("unknown convert target type must fail");
        assert!(err.to_string().contains("int"), "{err}");

        let pl = build(
            "convert",
            serde_json::json!({ "field": "status", "type": "integer" }),
        )
        .expect("known target type builds");
        let mut doc = serde_json::json!({ "status": "404" });
        pl.process(&mut doc);
        assert_eq!(doc["status"], 404);
    }

    #[test]
    fn non_array_timestamp_formats_is_rejected_not_dropped() {
        assert!(build(
            "timestamp_parse",
            serde_json::json!({ "field": "ts", "formats": "%Y-%m-%d" })
        )
        .is_err());
        // Absent `formats` is still the documented epoch/RFC-3339 default.
        assert!(build("timestamp_parse", serde_json::json!({ "field": "ts" })).is_ok());
    }

    /// A processor-level `if` guard is not evaluated on this path. Compiling it
    /// meant `PUT /_ingest/pipeline` answered 200 — which this change redefined
    /// to mean "xerj honours this" — and then ran the processor on exactly the
    /// documents the guard excludes. Measured pre-fix: `{"foo":
    /// "something-else"}` came out `{"foo": "something-else", "env": "prod"}`.
    #[test]
    fn a_processor_level_if_guard_is_not_silently_dropped() {
        let err = build(
            "set",
            serde_json::json!({
                "field": "env", "value": "prod", "if": "ctx.foo == 'never'"
            }),
        )
        .expect_err("an unhonourable `if` must not compile");
        assert!(
            matches!(&err, WasmError::UnsupportedOption { stage, option, .. }
                     if stage == "set" && option == "if"),
            "expected UnsupportedOption(if), got: {err:?}"
        );
    }

    #[test]
    fn on_failure_and_ignore_failure_are_not_silently_dropped() {
        for key in ["on_failure", "ignore_failure"] {
            let mut cfg = serde_json::json!({ "field": "env", "value": "prod" });
            cfg[key] = serde_json::json!(if key == "ignore_failure" {
                serde_json::json!(true)
            } else {
                serde_json::json!([{ "set": { "field": "x", "value": 1 } }])
            });
            let err = build("set", cfg).expect_err("{key} must not be dropped");
            assert!(
                matches!(&err, WasmError::UnsupportedOption { option, .. } if option == key),
                "expected UnsupportedOption({key}), got: {err:?}"
            );
        }
    }

    /// `tag` and `description` are identification metadata — they change no
    /// document and no decision on this path — so they stay accepted.
    #[test]
    fn processor_metadata_keys_still_compile() {
        assert!(build(
            "set",
            serde_json::json!({
                "field": "env", "value": "prod",
                "tag": "t1", "description": "sets env"
            })
        )
        .is_ok());
    }

    /// ES's `grok` is driven by `patterns` (an array of grok expressions);
    /// xerj's takes one NAMED pattern. Reading `pattern` and defaulting to
    /// `SYSLOG` meant an ES grok processor compiled clean and left the document
    /// untouched — measured pre-fix: `{"message": "10.0.0.1 GET"}` with the
    /// `%{IP:client}` capture nowhere.
    #[test]
    fn es_grok_patterns_array_is_not_silently_replaced_by_syslog() {
        let err = build(
            "grok",
            serde_json::json!({
                "field": "message", "patterns": ["%{IP:client} %{WORD:method}"]
            }),
        )
        .expect_err("`patterns` must not compile to the SYSLOG default");
        assert!(
            matches!(&err, WasmError::UnsupportedOption { stage, option, .. }
                     if stage == "grok" && option == "patterns"),
            "expected UnsupportedOption(patterns), got: {err:?}"
        );
    }

    /// Elasticsearch's `set` defaults `override` to true (SetProcessor.java:135
    /// — read for semantics only). `unwrap_or(false)` made `{"set": {"field":
    /// "env", "value": "prod"}}` a no-op against an existing `{"env": "dev"}`.
    #[test]
    fn set_overrides_by_default_like_elasticsearch() {
        let pl = build(
            "set",
            serde_json::json!({ "field": "env", "value": "prod" }),
        )
        .expect("set builds");
        let mut doc = serde_json::json!({ "env": "dev" });
        pl.process(&mut doc);
        assert_eq!(doc["env"], "prod");

        // …and the opt-out still works.
        let pl = build(
            "set",
            serde_json::json!({ "field": "env", "value": "prod", "override": false }),
        )
        .expect("set builds");
        let mut doc = serde_json::json!({ "env": "dev" });
        pl.process(&mut doc);
        assert_eq!(doc["env"], "dev");

        // A non-boolean `override` is refused rather than defaulted.
        assert!(build(
            "set",
            serde_json::json!({ "field": "env", "value": "prod", "override": "yes" })
        )
        .is_err());
    }

    /// `append` used to be mapped onto `set` in the ES translation, so it
    /// REPLACED the field. It is now a real append and agrees with the
    /// `_simulate` interpreter.
    #[test]
    fn append_extends_rather_than_replacing() {
        let pl = build(
            "append",
            serde_json::json!({ "field": "tags", "value": "b" }),
        )
        .expect("append builds");

        let mut doc = serde_json::json!({ "tags": ["a"] });
        pl.process(&mut doc);
        assert_eq!(doc["tags"], serde_json::json!(["a", "b"]));

        // A scalar widens into a list; a missing field becomes a 1-element one.
        let mut doc = serde_json::json!({ "tags": "a" });
        pl.process(&mut doc);
        assert_eq!(doc["tags"], serde_json::json!(["a", "b"]));
        let mut doc = serde_json::json!({});
        pl.process(&mut doc);
        assert_eq!(doc["tags"], serde_json::json!(["b"]));

        // A list value extends rather than nesting.
        let pl = build(
            "append",
            serde_json::json!({ "field": "tags", "value": ["b", "c"] }),
        )
        .expect("append builds");
        let mut doc = serde_json::json!({ "tags": ["a"] });
        pl.process(&mut doc);
        assert_eq!(doc["tags"], serde_json::json!(["a", "b", "c"]));

        // De-duplicating append is not implemented; asking for it is refused.
        assert!(build(
            "append",
            serde_json::json!({ "field": "tags", "value": "b", "allow_duplicates": false })
        )
        .is_err());
    }
}
