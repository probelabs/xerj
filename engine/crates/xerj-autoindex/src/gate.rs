//! The 10-minute gate: hand the decision back instead of taking the machine.
//!
//! `landing/llms.txt` already tells an agent driving XERJ on someone's laptop
//! to *estimate, tell them, ask if it is big, report progress*. That was
//! advice, and advice is ignorable. This module is the binary enforcing it:
//! when phase A's measured estimate ([`crate::estimate`]) exceeds
//! `--max-minutes` (default 10) and nobody has approved the run, autoindex
//! stops before it touches the index and emits a decision request instead.
//!
//! Three properties the design turns on:
//!
//! * **A distinct exit code.** [`EXIT_NEEDS_DECISION`] is 4, never 1. Exit 1
//!   is autoindex's catch-all for every real failure, so an agent that could
//!   only see 1 would have to parse prose to tell "your endpoint is down" from
//!   "I need you to choose". Anything that has to be distinguished
//!   programmatically gets its own code.
//! * **Never wait on stdin for a question the user cannot see.** The prompt is
//!   offered only when someone can answer it (stdin is a terminal) *and* the
//!   question actually reached a screen (stderr is a terminal, and the progress
//!   surface is not silenced). A run started by an agent, a CI job, a pipe, or
//!   with `--quiet` emits the payload and exits; waiting for an answer that
//!   cannot arrive is how an agent deadlocks, and an invisible prompt is
//!   indistinguishable from a hang — the worst possible failure for a feature
//!   whose whole purpose is to say "this is going to take a while".
//!   [`prompt_blocked_by`] owns that decision and [`answer_from_terminal`]
//!   never opens stdin when it says no.
//! * **Options carry their measured cost, or say they were not measured.**
//!   `narrower` names the heaviest directories in this corpus with real byte
//!   counts and re-costs the run without them using the same measured rates.
//!   `fast` does NOT claim a speed-up factor: `--no-semantic --no-graph`
//!   removes server-side embedding and client-side edge detection, and this
//!   run measured neither. It states the scope it changes and admits the
//!   factor is unmeasured, which is the only honest thing to print.

use crate::estimate::{human_bytes, human_secs, Estimate, MeasuredRate, PlannedFile};
use crate::order;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::IsTerminal;

/// "I need a decision before I spend your machine." Deliberately not 1 (every
/// error), not 2 (usage), not 3 (completed-with-junk).
pub const EXIT_NEEDS_DECISION: i32 = 4;

/// Default gate threshold in minutes, from the owner's own framing: "if
/// estimated more than 10min work needs to ask AI back what to do".
pub const DEFAULT_MAX_MINUTES: u64 = 10;

/// An answer to a decision request, carried by `--approve`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approval {
    /// Index everything as planned.
    Proceed,
    /// Index everything, but with `--no-semantic --no-graph`.
    Fast,
    /// Index nothing and exit cleanly.
    Cancel,
}

impl Approval {
    pub fn as_str(self) -> &'static str {
        match self {
            Approval::Proceed => "proceed",
            Approval::Fast => "fast",
            Approval::Cancel => "cancel",
        }
    }

    /// Parse an `--approve` value.
    ///
    /// `narrower` is refused on purpose. It is a real option in the decision
    /// request, but it is not something this flag can carry out: making a run
    /// narrower means pointing autoindex at a different folder, and accepting
    /// `--approve narrower` would mean accepting an instruction and then
    /// indexing the whole tree anyway — the accepted-and-silently-ignored
    /// class this repository keeps re-finding (#204).
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw {
            "proceed" => Ok(Approval::Proceed),
            "fast" => Ok(Approval::Fast),
            "cancel" => Ok(Approval::Cancel),
            "narrower" => Err(
                "--approve narrower cannot be honoured here: 'narrower' means running autoindex \
                 against a smaller folder, which this run cannot decide for you. Re-run it as \
                 `xerj autoindex <subdirectory>` (the decision request lists the heaviest \
                 directories and what each one costs)"
                    .to_string(),
            ),
            other => Err(format!(
                "--approve {other} is not one of: proceed, fast, cancel (narrower means re-running \
                 against a subdirectory — see the decision request)"
            )),
        }
    }
}

/// Is this run long enough to be worth a question?
///
/// Three ways to answer no, and each matters:
/// * `max_minutes == 0` — the documented opt-out. A caller that says "never
///   ask" is obeyed exactly.
/// * no estimate — phase A measured nothing it could price honestly, and a
///   gate cannot fire on a number that does not exist. Stopping a run on an
///   invented threshold breach would be the worst possible version of this
///   feature.
/// * under the threshold.
///
/// The comparison is against the **upper** bound: the estimate deliberately
/// leaves out server, network and embedding time, so its lower end is a floor
/// that would let long runs through unasked.
pub fn over_threshold(estimate: &Estimate, max_minutes: u64) -> bool {
    if max_minutes == 0 {
        return false;
    }
    estimate
        .gate_seconds()
        .is_some_and(|seconds| seconds > (max_minutes * 60) as f64)
}

/// Weight of one directory in the corpus — measured, never sampled.
#[derive(Debug, Clone, Serialize)]
pub struct DirectoryWeight {
    /// Root-relative directory, or `"."` for files directly under the root.
    pub path: String,
    pub files: u64,
    pub bytes: u64,
    /// Share of planned bytes, 0.0–1.0.
    pub share: f64,
    /// Why this path looks like it can be dropped, when it does.
    pub looks_generated: Option<String>,
}

/// Aggregate planned bytes by first path segment, heaviest first.
pub fn heaviest_directories(files: &[(String, u64)], top_n: usize) -> Vec<DirectoryWeight> {
    let total: u64 = files.iter().map(|(_, bytes)| bytes).sum();
    let mut by_dir: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
    for (rel, bytes) in files {
        let segment = rel.split_once('/').map(|(head, _)| head).unwrap_or(".");
        let entry = by_dir.entry(segment).or_default();
        entry.0 += 1;
        entry.1 += bytes;
    }
    let mut weights: Vec<DirectoryWeight> = by_dir
        .into_iter()
        .map(|(path, (files, bytes))| DirectoryWeight {
            path: path.to_string(),
            files,
            bytes,
            share: if total > 0 {
                bytes as f64 / total as f64
            } else {
                0.0
            },
            looks_generated: order::vendored_reason(&format!("{path}/x"))
                .map(|hit| format!("matches the vendored/generated rule '{hit}'")),
        })
        .collect();
    weights.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.path.cmp(&b.path)));
    weights.truncate(top_n);
    weights
}

/// Everything the caller has to know to answer, assembled once.
pub struct DecisionRequest<'a> {
    pub root: String,
    pub estimate: &'a Estimate,
    pub max_minutes: u64,
    pub bands: Vec<order::BandSummary>,
    pub heaviest: Vec<DirectoryWeight>,
    /// Estimate with every heaviest-directory entry that looks generated
    /// removed — costed with the same measured rates.
    pub without_generated: Option<(Vec<String>, String)>,
    /// Datasets phase A inferred a `semantic_text` field for. `fast` removes
    /// the server-side embedding work for exactly these.
    pub semantic_datasets: Vec<String>,
    pub total_datasets: usize,
    /// Files relationship detection would run over under the current flags.
    pub graph_files: u64,
    /// Why this run did not stop to ask at a terminal, when it did not.
    /// Carried in the payload because stdout is the one channel `--quiet`
    /// leaves open: a user who silenced stderr still has to be able to find
    /// out that a question existed and why they were not asked it.
    pub prompt_blocked: Option<NoPrompt>,
}

impl DecisionRequest<'_> {
    /// The machine-readable payload. One JSON document on stdout, so an agent
    /// never has to scrape prose.
    pub fn to_json(&self) -> Value {
        json!({
            "xerj": "autoindex-decision-request",
            "exit_code": EXIT_NEEDS_DECISION,
            "root": self.root,
            "reason": self.reason(),
            "estimate": self.estimate,
            "estimate_text": self.estimate.headline(),
            "max_minutes": self.max_minutes,
            "priority_order": self.bands,
            "heaviest_directories": self.heaviest,
            "options": self.options(),
            "prompt_offered": self.prompt_blocked.is_none(),
            "prompt_not_offered_because": self.prompt_blocked.map(NoPrompt::explain),
            "how_to_answer":
                "re-invoke the identical command with --approve <id> (--yes is an alias for \
                 --approve proceed). Without an answer nothing has been indexed and nothing has \
                 been changed on the server.",
        })
    }

    /// Why the run stopped, in one sentence, for both renderings.
    pub fn reason(&self) -> String {
        match self.estimate.gate_seconds() {
            Some(seconds) => format!(
                "the measured extraction floor alone is {}; its upper end {} already exceeds \
                 --max-minutes {}. The real run is longer than this — server, network and \
                 embedding time are not in the number",
                self.estimate.range_text(),
                human_secs(seconds),
                self.max_minutes
            ),
            None => "no estimate was possible".to_string(),
        }
    }

    fn options(&self) -> Value {
        let narrower = match &self.without_generated {
            Some((names, text)) => format!(
                "drop {} ({}) and the same measured rates put the run at {text}",
                names.join(", "),
                human_bytes(
                    self.heaviest
                        .iter()
                        .filter(|d| d.looks_generated.is_some())
                        .map(|d| d.bytes)
                        .sum::<u64>()
                ),
            ),
            None => "no single directory dominates this corpus".to_string(),
        };
        json!([
            {
                "id": "proceed",
                "invoke": "--approve proceed   (or --yes)",
                "effect": "index everything as planned",
                "cost": self.estimate.headline(),
            },
            {
                "id": "fast",
                "invoke": "--approve fast",
                "effect": "adds --no-semantic --no-graph: no semantic_text body fields, no \
                           relationship edges. You still get typed BM25 + keyword indices over \
                           the same files.",
                "cost": format!(
                    "speed-up NOT measured by this run. It removes server-side embedding for {} \
                     of {} inferred dataset(s) ({}) and relationship detection over {} file(s). \
                     XERJ's default embedder is lexical feature-hashing, so unless the node runs \
                     --embed-mode neural the embedding saving is smaller than the word 'semantic' \
                     suggests.",
                    self.semantic_datasets.len(),
                    self.total_datasets,
                    if self.semantic_datasets.is_empty() {
                        "none".to_string()
                    } else {
                        self.semantic_datasets.join(", ")
                    },
                    self.graph_files,
                ),
            },
            {
                "id": "narrower",
                "invoke": "re-run as `xerj autoindex <subdirectory>` — --approve cannot carry \
                           this out, autoindex has no --exclude flag",
                "effect": "index only the part that matters",
                "cost": narrower,
                "heaviest_directories": self.heaviest,
            },
            {
                "id": "cancel",
                "invoke": "--approve cancel",
                "effect": "index nothing, exit 0",
                "cost": "none — nothing has been written yet",
            },
        ])
    }

    /// The same information as prose, for a person reading a terminal.
    pub fn prose(&self) -> Vec<String> {
        let mut lines = vec![
            format!("autoindex: STOPPING BEFORE INDEXING — {}", self.reason()),
            format!("  root:     {}", self.root),
            format!(
                "  corpus:   {} files, {}",
                self.estimate.planned_files,
                human_bytes(self.estimate.planned_bytes)
            ),
            format!("  estimate: {}", self.estimate.headline()),
            format!("  basis:    {}", self.estimate.basis),
        ];
        for family in &self.estimate.families {
            lines.push(format!(
                "    {:<10} {:>6} files {:>10}  at {}/s measured over {} file(s)",
                family.family,
                family.planned_files,
                human_bytes(family.planned_bytes),
                human_bytes(family.bytes_per_sec as u64),
                family.measured_files,
            ));
        }
        for family in &self.estimate.unmeasured_families {
            lines.push(format!(
                "    {:<10} {:>6} files {:>10}  NOT in the estimate — {}",
                family.family,
                family.planned_files,
                human_bytes(family.planned_bytes),
                family.reason,
            ));
        }
        lines.push("  not included in the estimate:".to_string());
        for exclude in &self.estimate.excludes {
            lines.push(format!("    - {exclude}"));
        }
        if !self.heaviest.is_empty() {
            lines.push("  heaviest directories:".to_string());
            for dir in &self.heaviest {
                lines.push(format!(
                    "    {:<24} {:>6} files {:>10} ({:.0}%){}",
                    dir.path,
                    dir.files,
                    human_bytes(dir.bytes),
                    dir.share * 100.0,
                    match &dir.looks_generated {
                        Some(reason) => format!("  ← {reason}"),
                        None => String::new(),
                    }
                ));
            }
        }
        lines.push("  options:".to_string());
        let options = self.options();
        if let Some(options) = options.as_array() {
            for option in options {
                lines.push(format!(
                    "    {:<9} {}",
                    option["id"].as_str().unwrap_or_default(),
                    option["effect"].as_str().unwrap_or_default()
                ));
                lines.push(format!(
                    "              cost: {}",
                    option["cost"].as_str().unwrap_or_default()
                ));
            }
        }
        lines.push(
            "  answer by re-running the same command with --approve proceed|fast|cancel (--yes = \
             proceed). Nothing has been indexed."
                .to_string(),
        );
        // Say why nobody was asked, where that can still be read. A log that
        // shows the question but not the reason it went unasked is the same
        // guessing game one layer up.
        if let Some(blocked) = self.prompt_blocked {
            lines.push(format!("  {}", blocked.explain()));
        }
        lines
    }
}

/// Why no terminal prompt was offered. `None` from [`prompt_blocked_by`] means
/// one was.
///
/// This exists as a value rather than a bare `bool` because the reason is
/// printed: a run that stops without asking has to be able to say why it did
/// not ask, or the silence is just another thing for the user to guess at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoPrompt {
    /// stdin is a pipe, a file, or closed — an agent, a CI job, a script.
    /// Nobody is at the keyboard, so there is no answer to wait for.
    StdinNotTerminal,
    /// stderr is redirected. The question would land in a log nobody is
    /// watching while the process sat waiting for a reply to it.
    StderrNotTerminal,
    /// `--quiet` / `--progress none` silenced the surface the question is
    /// printed on. Someone *is* at the keyboard, but they were never shown
    /// anything to answer — this is the case that made the run look hung.
    QuestionSilenced,
}

impl NoPrompt {
    /// One sentence, for the decision payload and the prose.
    pub fn explain(self) -> &'static str {
        match self {
            NoPrompt::StdinNotTerminal => {
                "no terminal prompt: stdin is not a terminal, so no answer could be typed. Re-run \
                 with --approve proceed|fast|cancel"
            }
            NoPrompt::StderrNotTerminal => {
                "no terminal prompt: stderr is not a terminal, so the question would not have \
                 reached a screen. Re-run with --approve proceed|fast|cancel"
            }
            NoPrompt::QuestionSilenced => {
                "no terminal prompt: --quiet / --progress none silences the question, and this run \
                 will never wait on stdin for something it did not print. The decision request \
                 below is on stdout, which --quiet does not silence. Re-run with --approve \
                 proceed|fast|cancel, or without --quiet to be asked"
            }
        }
    }
}

/// Should this run stop to ask, and if not, why not?
///
/// Three conditions, and all three have to hold before autoindex touches
/// stdin:
///
/// * `stdin_tty` — someone can type an answer;
/// * `stderr_tty` — the question reaches a screen rather than a log file;
/// * `question_visible` — the progress surface actually printed it. This is
///   the one the first cut of the gate missed. `--quiet` / `--progress none`
///   routes [`crate::progress::Progress::note`] to nothing, so the prose and
///   the prompt vanished while the process still sat on `read_line`. Measured
///   before the fix: a pty run with `--progress none` printed **zero bytes**
///   and was still blocked at 220 s. An invisible prompt is a hang.
///
/// Pure, so the whole rule is testable without a terminal — the same shape as
/// [`crate::progress::resolve`].
pub fn prompt_blocked_by(
    stdin_tty: bool,
    stderr_tty: bool,
    question_visible: bool,
) -> Option<NoPrompt> {
    if !stdin_tty {
        return Some(NoPrompt::StdinNotTerminal);
    }
    if !stderr_tty {
        return Some(NoPrompt::StderrNotTerminal);
    }
    if !question_visible {
        return Some(NoPrompt::QuestionSilenced);
    }
    None
}

/// [`prompt_blocked_by`] against this process's real streams.
///
/// `question_visible` is the caller's progress surface
/// ([`crate::progress::Progress::enabled`]) — gate has no business inspecting
/// the surface itself, and passing the fact in keeps this testable.
pub fn detect_prompt_block(question_visible: bool) -> Option<NoPrompt> {
    prompt_blocked_by(
        std::io::stdin().is_terminal(),
        std::io::stderr().is_terminal(),
        question_visible,
    )
}

/// Ask, but only when [`prompt_blocked_by`] said it was safe to.
///
/// `open_stdin` is a closure and not a reader on purpose: when the prompt is
/// blocked it is **never called**, so there is no path on which this function
/// can end up holding — let alone reading — the process's stdin. That is the
/// invariant, and `a_question_that_cannot_be_seen_is_never_asked` asserts it by
/// handing in a reader that panics if anyone touches it.
pub fn answer_from_terminal<R: std::io::BufRead>(
    blocked: Option<NoPrompt>,
    open_stdin: impl FnOnce() -> R,
    echo: impl FnMut(&str),
) -> Option<Approval> {
    if blocked.is_some() {
        return None;
    }
    read_answer(&mut open_stdin(), echo)
}

/// Read one answer from a terminal. `None` means "no answer" — EOF, an
/// unreadable stdin, or three unrecognised replies — and the caller must treat
/// that as cancel rather than as consent.
pub fn read_answer(
    input: &mut impl std::io::BufRead,
    mut echo: impl FnMut(&str),
) -> Option<Approval> {
    for _ in 0..3 {
        echo("  answer [p]roceed / [f]ast / [c]ancel (narrower = re-run on a subdirectory): ");
        let mut line = String::new();
        match input.read_line(&mut line) {
            Ok(0) | Err(_) => return None,
            Ok(_) => {}
        }
        match line.trim().to_ascii_lowercase().as_str() {
            "p" | "proceed" | "y" | "yes" => return Some(Approval::Proceed),
            "f" | "fast" => return Some(Approval::Fast),
            "c" | "cancel" | "n" | "no" | "q" => return Some(Approval::Cancel),
            "" => echo("  no answer read; type p, f or c"),
            other => echo(&format!("  '{other}' is not one of p, f, c")),
        }
    }
    None
}

/// Cost the run again with the generated-looking heavy directories removed —
/// same measured rates, no new guess. `None` when nothing looks droppable.
pub fn without_generated_directories(
    estimate: &Estimate,
    rates: &BTreeMap<&'static str, MeasuredRate>,
    heaviest: &[DirectoryWeight],
    files: &[(String, String, u64)],
) -> Option<(Vec<String>, String)> {
    let dropped: Vec<String> = heaviest
        .iter()
        .filter(|dir| dir.looks_generated.is_some())
        .map(|dir| dir.path.clone())
        .collect();
    if dropped.is_empty() {
        return None;
    }
    let kept: Vec<PlannedFile> = files
        .iter()
        .filter(|(rel, _, _)| {
            let segment = rel.split_once('/').map(|(head, _)| head).unwrap_or(".");
            !dropped.iter().any(|d| d == segment)
        })
        .map(|(_, family, bytes)| PlannedFile {
            family: family.clone(),
            bytes: *bytes,
        })
        .collect();
    Some((
        dropped,
        estimate.recompute_subset(&kept, rates).range_text(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rates() -> BTreeMap<&'static str, MeasuredRate> {
        BTreeMap::from([(
            "code",
            MeasuredRate {
                files: 4,
                bytes: 4_000,
                bytes_per_sec: 1_000.0,
            },
        )])
    }

    fn estimate(files: usize, bytes: u64) -> Estimate {
        let planned: Vec<PlannedFile> = (0..files)
            .map(|_| PlannedFile {
                family: "code".into(),
                bytes,
            })
            .collect();
        Estimate::compute(&planned, &rates(), 4)
    }

    /// The point of the whole module: "needs a decision" must be
    /// distinguishable from "something broke" without parsing English.
    #[test]
    fn the_decision_exit_code_is_not_any_existing_one() {
        for taken in [0, 1, 2, 3] {
            assert_ne!(EXIT_NEEDS_DECISION, taken);
        }
    }

    /// The threshold rule itself, including the two ways it must decline to
    /// fire. A gate that stopped a run on a number nobody measured would be
    /// worse than no gate.
    #[test]
    fn the_threshold_fires_on_the_upper_bound_and_never_on_an_absent_one() {
        // 100 files × 1000 B at 1000 B/s over 4 workers = 25 s of work,
        // longest job 1 s → high ≈ 25.75 s.
        let quick = estimate(100, 1_000);
        assert!(quick.high_seconds.unwrap() < 60.0);
        assert!(!over_threshold(&quick, 1));

        // 100 files × 20 kB at the same rate = 20 s each, 2000 s of work over
        // 4 workers → 500 s + a 15 s tail: past one minute, under ten.
        let slow = estimate(100, 20_000);
        let high = slow.high_seconds.unwrap();
        assert!(high > 60.0 && high < 600.0, "{high}");
        assert!(over_threshold(&slow, 1));
        assert!(!over_threshold(&slow, 10));

        // The documented opt-out is obeyed exactly.
        assert!(!over_threshold(&slow, 0));

        // No measurement, no gate — under any threshold.
        let unmeasured = Estimate::compute(
            &[PlannedFile {
                family: "sqlite".into(),
                bytes: 1 << 40,
            }],
            &BTreeMap::new(),
            4,
        );
        assert!(unmeasured.gate_seconds().is_none());
        for minutes in [1, 10, 10_080] {
            assert!(!over_threshold(&unmeasured, minutes));
        }
    }

    #[test]
    fn approve_accepts_the_three_it_can_carry_out_and_refuses_the_one_it_cannot() {
        assert_eq!(Approval::parse("proceed"), Ok(Approval::Proceed));
        assert_eq!(Approval::parse("fast"), Ok(Approval::Fast));
        assert_eq!(Approval::parse("cancel"), Ok(Approval::Cancel));
        // #204: accepting this and indexing everything anyway is the bug.
        let refused = Approval::parse("narrower").unwrap_err();
        assert!(refused.contains("cannot be honoured"), "{refused}");
        assert!(refused.contains("subdirectory"), "{refused}");
        let unknown = Approval::parse("later").unwrap_err();
        assert!(unknown.contains("proceed, fast, cancel"), "{unknown}");
        // Case and spelling are not guessed at.
        assert!(Approval::parse("Proceed").is_err());
        assert!(Approval::parse("").is_err());
    }

    #[test]
    fn the_payload_carries_the_estimate_the_counts_the_bands_and_the_options() {
        let estimate = estimate(10, 1_000);
        let request = DecisionRequest {
            root: "/data".into(),
            estimate: &estimate,
            max_minutes: 10,
            bands: order::summarize(&[order::Item {
                index: 0,
                band: order::Band::SourceAndDocs,
                bytes: 10_000,
            }]),
            heaviest: heaviest_directories(
                &[
                    ("node_modules/a.js".to_string(), 9_000),
                    ("src/main.rs".to_string(), 1_000),
                ],
                5,
            ),
            without_generated: Some((vec!["node_modules".into()], "1.0 s–1.0 s".into())),
            semantic_datasets: vec!["docs".into()],
            total_datasets: 3,
            graph_files: 10,
            prompt_blocked: None,
        };
        let payload = request.to_json();
        assert_eq!(payload["xerj"], "autoindex-decision-request");
        assert_eq!(payload["exit_code"], EXIT_NEEDS_DECISION);
        assert_eq!(payload["estimate"]["planned_files"], 10);
        // An agent reading this must not be able to mistake the floor for a
        // prediction of the run — the machine-readable kind says which it is,
        // and the human-readable line repeats it in words.
        assert_eq!(
            payload["estimate"]["kind"],
            crate::estimate::ESTIMATE_KIND,
            "the payload must name what kind of number this is"
        );
        let estimate_text = payload["estimate_text"].as_str().unwrap();
        assert!(estimate_text.starts_with("at least "), "{estimate_text}");
        assert!(estimate_text.contains("MEASURED FLOOR"), "{estimate_text}");
        assert!(
            payload["reason"]
                .as_str()
                .unwrap()
                .contains("The real run is longer than this"),
            "{}",
            payload["reason"]
        );
        assert_eq!(payload["estimate"]["planned_bytes"], 10_000);
        assert_eq!(payload["priority_order"][0]["band"], "source-and-docs");
        assert!(payload["priority_order"][0]["why"]
            .as_str()
            .unwrap()
            .contains("searches for first"));

        let options = payload["options"].as_array().unwrap();
        let ids: Vec<&str> = options.iter().map(|o| o["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["proceed", "fast", "narrower", "cancel"]);
        // The honest-claims rule, asserted: `fast` states its scope and admits
        // the factor is not measured, instead of printing an invented "3×".
        let fast = options[1]["cost"].as_str().unwrap();
        assert!(fast.contains("NOT measured"), "{fast}");
        assert!(fast.contains("1 of 3 inferred dataset(s)"), "{fast}");
        assert!(fast.contains("lexical feature-hashing"), "{fast}");
        // `narrower` names the directory and re-costs it with measured rates.
        let narrower = options[2]["cost"].as_str().unwrap();
        assert!(narrower.contains("node_modules"), "{narrower}");
        // …and it never claims --approve can do it.
        assert!(options[2]["invoke"]
            .as_str()
            .unwrap()
            .contains("subdirectory"));

        // The prose carries the same facts for a human.
        let prose = request.prose().join("\n");
        assert!(prose.contains("STOPPING BEFORE INDEXING"));
        assert!(prose.contains("node_modules"));
        assert!(prose.contains("Nothing has been indexed."));
        assert!(prose.contains("not included in the estimate:"));
    }

    #[test]
    fn the_heaviest_directories_are_measured_and_flagged() {
        let dirs = heaviest_directories(
            &[
                ("node_modules/a/b.js".to_string(), 700),
                ("node_modules/c.js".to_string(), 100),
                ("src/main.rs".to_string(), 150),
                ("README.md".to_string(), 50),
            ],
            5,
        );
        assert_eq!(dirs[0].path, "node_modules");
        assert_eq!((dirs[0].files, dirs[0].bytes), (2, 800));
        assert!((dirs[0].share - 0.8).abs() < 1e-9);
        assert!(dirs[0].looks_generated.is_some());
        assert_eq!(dirs[1].path, "src");
        assert!(dirs[1].looks_generated.is_none());
        // Root-level files are their own bucket, not silently merged.
        assert_eq!(dirs[2].path, ".");
        assert_eq!(dirs[2].bytes, 50);
        assert!(heaviest_directories(&[], 5).is_empty());
    }

    #[test]
    fn dropping_the_generated_directories_is_recosted_not_guessed() {
        let files = vec![
            (
                "node_modules/a.js".to_string(),
                "code".to_string(),
                9_000u64,
            ),
            ("src/main.rs".to_string(), "code".to_string(), 1_000),
        ];
        let planned: Vec<PlannedFile> = files
            .iter()
            .map(|(_, family, bytes)| PlannedFile {
                family: family.clone(),
                bytes: *bytes,
            })
            .collect();
        let estimate = Estimate::compute(&planned, &rates(), 1);
        let heaviest = heaviest_directories(
            &files
                .iter()
                .map(|(rel, _, bytes)| (rel.clone(), *bytes))
                .collect::<Vec<_>>(),
            5,
        );
        let (dropped, text) =
            without_generated_directories(&estimate, &rates(), &heaviest, &files).unwrap();
        assert_eq!(dropped, ["node_modules"]);
        // 1000 bytes at the measured 1000 B/s on one worker = 1.0 s.
        assert!(text.starts_with("1.0 s–1.0 s"), "{text}");
        // Nothing droppable → no invented saving.
        let clean = vec![("src/main.rs".to_string(), "code".to_string(), 1_000u64)];
        let clean_dirs = heaviest_directories(&[("src/main.rs".to_string(), 1_000)], 5);
        assert!(without_generated_directories(&estimate, &rates(), &clean_dirs, &clean).is_none());
    }

    /// A non-answer is a cancel. Treating EOF as consent would turn every
    /// piped run into an unattended full index — the exact outcome the gate
    /// exists to prevent.
    #[test]
    fn an_unanswerable_prompt_never_returns_consent() {
        let mut echoed = Vec::new();
        let answer = read_answer(&mut std::io::Cursor::new(b""), |line| {
            echoed.push(line.to_string())
        });
        assert_eq!(answer, None);
        assert_eq!(
            echoed.len(),
            1,
            "EOF must not re-prompt forever: {echoed:?}"
        );

        // Three unrecognised answers and it gives up rather than looping.
        let mut count = 0;
        let answer = read_answer(&mut std::io::Cursor::new(b"x\ny\n\nz\nq\n"), |_| count += 1);
        // 'y' is an accepted spelling of proceed, so this stops at line 2.
        assert_eq!(answer, Some(Approval::Proceed));
        assert!(count >= 2);

        let mut noise = 0;
        assert_eq!(
            read_answer(&mut std::io::Cursor::new(b"a\nb\nc0\nd\n"), |_| noise += 1),
            None
        );
    }

    /// A reader that fails the test if anything reads it. Standing in for the
    /// process's real stdin, which has no EOF and no answer coming.
    struct NeverRead;

    impl std::io::Read for NeverRead {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            panic!("stdin was read for a question the user was never shown");
        }
    }

    impl std::io::BufRead for NeverRead {
        fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
            panic!("stdin was read for a question the user was never shown");
        }
        fn consume(&mut self, _: usize) {}
    }

    /// THE regression test.
    ///
    /// `--quiet` / `--progress none` silences `Progress::note`, which is where
    /// both the gate's prose and its prompt are printed — but the first cut of
    /// the gate decided whether to ask from the two tty checks alone. So a
    /// person at a terminal who passed `--quiet` was asked a question that was
    /// never printed, and the process sat on `read_line` forever. Measured on
    /// the pre-fix binary: a pty run with `--progress none` over the threshold
    /// emitted **0 bytes** and was still blocked when it was killed at 220 s.
    /// From the outside that is indistinguishable from a hang, in the one
    /// feature whose entire job is to say "this will take a while".
    ///
    /// The reader panics if it is touched, so the invariant asserted here is
    /// the real one — not "an answer was not returned" but "stdin was not
    /// read".
    #[test]
    fn a_question_that_cannot_be_seen_is_never_asked() {
        // Someone IS at the keyboard and both streams are terminals — the one
        // thing missing is that the surface printed nothing.
        let blocked = prompt_blocked_by(true, true, false);
        assert_eq!(
            blocked,
            Some(NoPrompt::QuestionSilenced),
            "a silenced surface must block the prompt on its own"
        );
        let mut echoed = Vec::new();
        // Panics unless the fix is present: pre-fix this opened and read stdin.
        let answer =
            answer_from_terminal(blocked, || NeverRead, |line| echoed.push(line.to_string()));
        assert_eq!(answer, None, "an unaskable question is never consent");
        assert!(
            echoed.is_empty(),
            "nothing may be echoed to a surface that prints nothing: {echoed:?}"
        );

        // …and the run still says so where it can be read. stdout is not
        // silenced by --quiet, so the payload has to carry the reason.
        let estimate = estimate(10, 1_000);
        let request = DecisionRequest {
            root: "/data".into(),
            estimate: &estimate,
            max_minutes: 1,
            bands: Vec::new(),
            heaviest: Vec::new(),
            without_generated: None,
            semantic_datasets: Vec::new(),
            total_datasets: 1,
            graph_files: 0,
            prompt_blocked: blocked,
        };
        let payload = request.to_json();
        assert_eq!(payload["prompt_offered"], false);
        let why = payload["prompt_not_offered_because"].as_str().unwrap();
        assert!(why.contains("--quiet"), "{why}");
        assert!(why.contains("stdout"), "{why}");
        assert!(why.contains("--approve"), "{why}");
        // The exit code is unchanged: a quiet run behaves exactly like the
        // agent-driven one it now matches.
        assert_eq!(payload["exit_code"], EXIT_NEEDS_DECISION);
    }

    /// The other three ways in, so the rule is pinned end to end rather than
    /// only at the case that was broken.
    #[test]
    fn the_prompt_is_offered_only_when_all_three_conditions_hold() {
        // A terminal, a visible surface: this is the one case that asks.
        assert_eq!(prompt_blocked_by(true, true, true), None);
        // Agent, CI, pipe: nobody to type. Checked first because it is the
        // reason that stays true however the surface is configured.
        assert_eq!(
            prompt_blocked_by(false, true, true),
            Some(NoPrompt::StdinNotTerminal)
        );
        assert_eq!(
            prompt_blocked_by(false, false, false),
            Some(NoPrompt::StdinNotTerminal)
        );
        // stderr redirected to a log: the question would not reach a screen.
        assert_eq!(
            prompt_blocked_by(true, false, true),
            Some(NoPrompt::StderrNotTerminal)
        );
        // Every reason names a way out, and none of them is "wait".
        for reason in [
            NoPrompt::StdinNotTerminal,
            NoPrompt::StderrNotTerminal,
            NoPrompt::QuestionSilenced,
        ] {
            let text = reason.explain();
            assert!(text.contains("--approve"), "{text}");
            // Every blocked path must leave stdin alone, not merely decline to
            // return an answer.
            assert_eq!(
                answer_from_terminal(Some(reason), || NeverRead, |_| {}),
                None
            );
        }

        // A visible terminal really does read the answer — the fix must not
        // have turned the prompt off for everyone.
        assert_eq!(
            answer_from_terminal(None, || std::io::Cursor::new(b"f\n"), |_| {}),
            Some(Approval::Fast)
        );
    }

    #[test]
    fn every_spelling_of_an_answer_maps_to_one_decision() {
        for (input, expected) in [
            ("p\n", Approval::Proceed),
            ("PROCEED\n", Approval::Proceed),
            ("  yes  \n", Approval::Proceed),
            ("f\n", Approval::Fast),
            ("Fast\n", Approval::Fast),
            ("c\n", Approval::Cancel),
            ("no\n", Approval::Cancel),
            ("q\n", Approval::Cancel),
        ] {
            assert_eq!(
                read_answer(&mut std::io::Cursor::new(input.as_bytes()), |_| {}),
                Some(expected),
                "{input:?}"
            );
        }
    }
}
