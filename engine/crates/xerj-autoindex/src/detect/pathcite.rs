//! pathcite@1 — a file path mentioned in prose ("see `src/index.rs:123`")
//! that resolves to a corpus file.
//!
//! Why it exists: on a real repository almost no doc→code citation is a
//! markdown link — authors write bare paths. Measured on this repository,
//! mdlink caught 2 of ~944 doc→code path mentions; pathcite is the detector
//! for the other 99.8%. Weight 0.6 / confidence 0.7 — an authored citation,
//! but resolved by string matching against the corpus tree, one rung less
//! deliberate than a link the author formatted as a link.
//!
//! ## Resolution (deterministic, never guessed)
//! Tokens are maximal `[A-Za-z0-9_./-]` runs ending in `.<alnum ext>`, with
//! any leading `/`, `./`, `../` stripped (a trailing `:line` never enters the
//! token — `:` is outside the class). Then:
//! 1. exact rel-path match wins outright;
//! 2. tokens with ≥ 2 path segments may match as a path SUFFIX
//!    (`xerj-engine/src/index.rs` matches `engine/crates/xerj-engine/src/index.rs`);
//!    several candidates → lexicographically smallest rel, counted ambiguous;
//! 3. bare one-segment names (`index.rs`) are NEVER suffix-matched — if any
//!    corpus file bears the name, the mention is counted ambiguous, not
//!    linked. A bare filename is too weakly targeted to assert a belief over.
//!
//! Multi-segment tokens that match nothing are counted unresolved (a dangling
//! path citation is a fact worth surfacing); bare tokens that match nothing
//! are ignored — dotted prose ("e.g", domains, version numbers) is not a
//! citation and would flood the honesty counters with non-facts.
//!
//! A file citing itself is skipped silently: "cites file" pointing at the
//! file it appears in tells a reader nothing.
//!
//! Known overlap: the target of a markdown link is also a path token, so a
//! `[guide](docs/guide.md)` line yields both an mdlink and a pathcite edge.
//! Parallel edges of different types are legal (spec §2.3) and each carries
//! its own honest detector attribution; deduplicating would hide that the
//! mdlink signal is the stronger one.

use super::{
    line_at, CorpusFile, CorpusIndex, DetectorCounters, EdgeDetector, EdgeDraft, SectionCtx,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

pub const TAG: &str = "pathcite@1";
pub const EDGE_TYPE: &str = "pathcite";
pub const WEIGHT: f32 = 0.6;
pub const CONFIDENCE: f32 = 0.7;

#[derive(Default)]
pub struct Pathcite {
    unresolved: AtomicU64,
    ambiguous: AtomicU64,
}

fn token_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r"[A-Za-z0-9_./\-]+\.[A-Za-z0-9_]+").expect("static path-token regex")
    })
}

enum Resolved<'a> {
    Hit(&'a CorpusFile),
    Ambiguous(&'a CorpusFile),
    /// ≥2 segments, no corpus match — a dangling path citation, counted.
    Miss,
    /// One bare segment naming ≥1 corpus file — refused, counted ambiguous.
    BareRefused,
    /// Not recognizably a path citation at all — ignored, not counted.
    Noise,
}

fn resolve<'a>(corpus: &'a CorpusIndex, token: &str) -> Resolved<'a> {
    if let Some(f) = corpus.files.get(token) {
        return Resolved::Hit(f);
    }
    let segments = token.split('/').filter(|s| !s.is_empty()).count();
    let name = token.rsplit('/').next().unwrap_or(token);
    let candidates = corpus.by_name.get(name);
    if segments >= 2 {
        let suffix = format!("/{token}");
        let mut hits = candidates
            .into_iter()
            .flatten()
            .filter(|rel| rel.ends_with(&suffix));
        return match (hits.next(), hits.next()) {
            // Sorted vec ⇒ the first suffix hit IS the smallest rel.
            (Some(rel), None) => Resolved::Hit(&corpus.files[rel]),
            (Some(rel), Some(_)) => Resolved::Ambiguous(&corpus.files[rel]),
            (None, _) => Resolved::Miss,
        };
    }
    match candidates {
        Some(rels) if !rels.is_empty() => Resolved::BareRefused,
        _ => Resolved::Noise,
    }
}

impl EdgeDetector for Pathcite {
    fn tag(&self) -> &'static str {
        TAG
    }

    fn detect_text(&self, ctx: &SectionCtx<'_>, out: &mut Vec<EdgeDraft>) {
        for m in token_re().find_iter(ctx.text) {
            let mut token = m.as_str();
            loop {
                let trimmed = token
                    .trim_start_matches('/')
                    .trim_start_matches("./")
                    .trim_start_matches("../");
                if trimmed.len() == token.len() {
                    break;
                }
                token = trimmed;
            }
            if token.is_empty() {
                continue;
            }
            let offset = (m.start() + (m.as_str().len() - token.len())) as u64;
            let target = match resolve(ctx.corpus, token) {
                Resolved::Hit(f) => f,
                Resolved::Ambiguous(f) => {
                    self.ambiguous.fetch_add(1, Ordering::Relaxed);
                    f
                }
                Resolved::Miss => {
                    self.unresolved.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                Resolved::BareRefused => {
                    self.ambiguous.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                Resolved::Noise => continue,
            };
            if target.rel == ctx.file.rel {
                continue; // self-citation says nothing
            }
            out.push(EdgeDraft {
                src: ctx.section_doc_id.to_string(),
                dst: target.anchor_doc_id.clone(),
                edge_type: EDGE_TYPE,
                weight: WEIGHT,
                confidence: CONFIDENCE,
                valid_at_ms: ctx.file.mtime_ms,
                src_file: ctx.file.rel.clone(),
                quote: line_at(ctx.text, m.start()),
                offset,
                src_format: ctx.file.format.clone(),
                dst_format: target.format.clone(),
            });
        }
    }

    fn counters(&self) -> DetectorCounters {
        DetectorCounters {
            unresolved: self.unresolved.load(Ordering::Relaxed),
            ambiguous: self.ambiguous.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{corpus_file, CorpusIndex, SectionCtx};
    use super::*;

    fn corpus() -> CorpusIndex {
        CorpusIndex::build(vec![
            corpus_file("docs/notes.md", "k0", "d", "txt-prose", 10),
            corpus_file(
                "engine/crates/xerj-engine/src/index.rs",
                "k1",
                "d",
                "txt-lines",
                10,
            ),
            corpus_file(
                "engine/crates/xerj-fts/src/index.rs",
                "k2",
                "d",
                "txt-lines",
                10,
            ),
            corpus_file(
                "engine/crates/xerj-fts/src/lib.rs",
                "k3",
                "d",
                "txt-lines",
                10,
            ),
        ])
    }

    fn run(corpus: &CorpusIndex, text: &str) -> (Vec<EdgeDraft>, DetectorCounters) {
        let det = Pathcite::default();
        let ctx = SectionCtx {
            corpus,
            file: &corpus.files["docs/notes.md"],
            section_label: "section 0",
            prev_section: None,
            section_doc_id: "sec0",
            text,
        };
        let mut out = Vec::new();
        det.detect_text(&ctx, &mut out);
        (out, det.counters())
    }

    #[test]
    fn exact_and_suffix_paths_resolve_with_line_stripped() {
        let corpus = corpus();
        let (out, c) = run(
            &corpus,
            "See engine/crates/xerj-fts/src/lib.rs:42 and xerj-engine/src/index.rs for details.",
        );
        assert_eq!(out.len(), 2);
        assert_eq!(
            out[0].dst,
            corpus.files["engine/crates/xerj-fts/src/lib.rs"].anchor_doc_id
        );
        assert_eq!(out[0].offset, 4);
        assert_eq!(
            out[1].dst, corpus.files["engine/crates/xerj-engine/src/index.rs"].anchor_doc_id,
            "multi-segment suffix match"
        );
        assert_eq!(out[0].src_format, "md");
        assert_eq!(out[0].dst_format, "rs");
        assert_eq!(c.unresolved, 0);
        assert_eq!(c.ambiguous, 0);
    }

    #[test]
    fn bare_filenames_are_refused_and_counted_never_guessed() {
        let corpus = corpus();
        // index.rs names TWO corpus files; lib.rs names one — both are bare
        // single-segment mentions and neither may become an edge.
        let (out, c) = run(&corpus, "open index.rs then lib.rs");
        assert!(out.is_empty());
        assert_eq!(c.ambiguous, 2);
        assert_eq!(c.unresolved, 0);
    }

    #[test]
    fn ambiguous_suffix_takes_smallest_rel_and_counts() {
        let corpus = corpus();
        let (out, c) = run(&corpus, "grep src/index.rs for the loop");
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].dst, corpus.files["engine/crates/xerj-engine/src/index.rs"].anchor_doc_id,
            "lexicographically smallest rel wins deterministically"
        );
        assert_eq!(c.ambiguous, 1);
    }

    #[test]
    fn dangling_multi_segment_counts_and_prose_noise_is_ignored() {
        let corpus = corpus();
        let (out, c) = run(
            &corpus,
            "e.g. version 1.2.3 at https://example.com/missing/file.md — gone/nowhere.rs too",
        );
        assert!(out.is_empty());
        // example.com/missing/file.md and gone/nowhere.rs are dangling path
        // citations; "e.g", "1.2.3", "https" fragments are noise, not counted.
        assert_eq!(c.unresolved, 2);
        assert_eq!(c.ambiguous, 0);
    }

    #[test]
    fn self_citations_emit_nothing() {
        let corpus = corpus();
        let (out, c) = run(&corpus, "this file is docs/notes.md in the tree");
        assert!(out.is_empty());
        assert_eq!(c.unresolved, 0);
        assert_eq!(c.ambiguous, 0);
    }

    #[test]
    fn leading_dot_and_slash_prefixes_resolve() {
        let corpus = corpus();
        let (out, _) = run(&corpus, "run ./engine/crates/xerj-fts/src/lib.rs first");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].offset, 6, "offset points at the path, not the ./");
    }
}
