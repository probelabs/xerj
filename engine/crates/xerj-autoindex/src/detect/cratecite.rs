//! cratecite@1 — a crate directory named in prose ("BM25 (xerj-fts)") links
//! to that crate's `Cargo.toml` file card.
//!
//! The cross-type bridge for doc/PDF → code: marketing papers and design docs
//! name crates, not file paths. The crate table is deterministic Phase-A
//! structure — every corpus directory that directly contains a `Cargo.toml`,
//! keyed by the DIRECTORY basename (contents are never read, so this is
//! honestly "the folder named xerj-fts", not the `[package] name`, and the
//! module says so). Weight 0.5 / confidence 0.6 — a name-drop in prose is
//! authored but the least targeted citation in the ladder.
//!
//! Only crate names containing `-` or `_` are citable: a crate directory
//! named `server` would otherwise turn every English use of the word into an
//! edge. On multi-word names ("xerj-fts") a whole-word match is a real
//! reference with near-zero false-positive surface. Word boundaries exclude
//! `[A-Za-z0-9_\-./]` so `xerj-fts-extra` and path contexts
//! (`crates/xerj-fts/src/…`) do NOT match — path mentions are pathcite's job.
//! Underscore spellings (`xerj_fts`) are NOT matched — that is the import
//! name, not the directory name, and guessing the equivalence would be a
//! resolution rule this detector cannot verify.

use super::{line_at, DetectorCounters, EdgeDetector, EdgeDraft, SectionCtx};
use std::sync::atomic::{AtomicU64, Ordering};

pub const TAG: &str = "cratecite@1";
pub const EDGE_TYPE: &str = "cratecite";
pub const WEIGHT: f32 = 0.5;
pub const CONFIDENCE: f32 = 0.6;

#[derive(Default)]
pub struct Cratecite {
    ambiguous: AtomicU64,
}

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/')
}

/// Right-side boundary: a following `.` only breaks the match when it starts
/// an extension-like continuation ("xerj-fts.txt") — a sentence-final period
/// ("…feeds xerj-fts.") is ordinary prose and must not suppress the citation.
fn boundary_after(rest: &str) -> bool {
    let mut it = rest.chars();
    match it.next() {
        None => true,
        Some('.') => !it
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_'),
        Some(c) => !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '/')),
    }
}

fn citable(name: &str) -> bool {
    name.contains('-') || name.contains('_')
}

impl EdgeDetector for Cratecite {
    fn tag(&self) -> &'static str {
        TAG
    }

    fn detect_text(&self, ctx: &SectionCtx<'_>, out: &mut Vec<EdgeDraft>) {
        for (name, rels) in &ctx.corpus.crate_dirs {
            if !citable(name) {
                continue;
            }
            let mut from = 0usize;
            while let Some(found) = ctx.text[from..].find(name.as_str()) {
                let pos = from + found;
                from = pos + name.len();
                let before_ok = ctx.text[..pos]
                    .chars()
                    .next_back()
                    .is_none_or(|c| !is_word(c));
                let after_ok = boundary_after(&ctx.text[pos + name.len()..]);
                if !before_ok || !after_ok {
                    continue;
                }
                // rels is sorted: the first IS the smallest; >1 dirs sharing a
                // basename is counted, never silently guessed.
                if rels.len() > 1 {
                    self.ambiguous.fetch_add(1, Ordering::Relaxed);
                }
                let target = &ctx.corpus.files[&rels[0]];
                if target.rel == ctx.file.rel {
                    continue;
                }
                out.push(EdgeDraft {
                    src: ctx.section_doc_id.to_string(),
                    dst: target.anchor_doc_id.clone(),
                    edge_type: EDGE_TYPE,
                    weight: WEIGHT,
                    confidence: CONFIDENCE,
                    valid_at_ms: ctx.file.mtime_ms,
                    src_file: ctx.file.rel.clone(),
                    quote: line_at(ctx.text, pos),
                    offset: pos as u64,
                    src_format: ctx.file.format.clone(),
                    dst_format: target.format.clone(),
                });
            }
        }
    }

    fn counters(&self) -> DetectorCounters {
        DetectorCounters {
            unresolved: 0,
            ambiguous: self.ambiguous.load(Ordering::Relaxed),
            capped: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{corpus_file, CorpusIndex, SectionCtx};
    use super::*;

    fn corpus() -> CorpusIndex {
        CorpusIndex::build(vec![
            corpus_file("gtm/brief.pdf", "k0", "d", "pdf", 10),
            corpus_file(
                "engine/crates/xerj-fts/Cargo.toml",
                "k1",
                "d",
                "txt-lines",
                10,
            ),
            corpus_file(
                "engine/crates/xerj-engine/Cargo.toml",
                "k2",
                "d",
                "txt-lines",
                10,
            ),
            corpus_file("engine/Cargo.toml", "k3", "d", "txt-lines", 10),
        ])
    }

    fn run(corpus: &CorpusIndex, text: &str) -> (Vec<EdgeDraft>, DetectorCounters) {
        let det = Cratecite::default();
        let ctx = SectionCtx {
            corpus,
            file: &corpus.files["gtm/brief.pdf"],
            section_label: "page 1 section 0",
            prev_section: None,
            section_doc_id: "sec0",
            text,
        };
        let mut out = Vec::new();
        det.detect_text(&ctx, &mut out);
        (out, det.counters())
    }

    #[test]
    fn prose_mention_links_the_crates_cargo_toml_card() {
        let corpus = corpus();
        let (out, c) = run(
            &corpus,
            "Ranking is classic BM25 (xerj-fts) under the hood.",
        );
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].dst,
            corpus.files["engine/crates/xerj-fts/Cargo.toml"].anchor_doc_id
        );
        assert_eq!(
            out[0].quote,
            "Ranking is classic BM25 (xerj-fts) under the hood."
        );
        assert_eq!(out[0].offset, 25);
        assert_eq!(out[0].src_format, "pdf");
        assert_eq!(out[0].dst_format, "toml");
        assert_eq!(c.ambiguous, 0);
    }

    #[test]
    fn word_boundaries_exclude_longer_names_and_path_contexts() {
        let corpus = corpus();
        let (out, _) = run(
            &corpus,
            "xerj-fts-extra is not it; engine/crates/xerj-fts/src/lib.rs is a path; xerj-fts wins",
        );
        assert_eq!(out.len(), 1, "only the standalone whole-word mention");
        assert_eq!(out[0].offset, 71);
    }

    #[test]
    fn single_word_crate_dirs_are_not_citable() {
        // "engine" contains Cargo.toml but is a dictionary word — matching it
        // in prose would fabricate edges out of ordinary English.
        let corpus = corpus();
        let (out, _) = run(&corpus, "the engine restarts nightly");
        assert!(out.is_empty());
        assert!(corpus.crate_dirs.contains_key("engine"));
    }

    #[test]
    fn multiple_mentions_each_emit() {
        let corpus = corpus();
        let (out, _) = run(&corpus, "xerj-fts feeds xerj-engine.\nxerj-fts again.");
        assert_eq!(out.len(), 3);
    }
}
