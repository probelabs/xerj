//! wikilink@1 — `[[Target]]` / `[[Target|alias]]` in section text.
//!
//! The strongest signal the detectors have: a human deliberately named
//! another note. Hence weight 1.0; confidence 0.95 rather than 1.0 because
//! resolution is by name, and a corpus can hold two files answering to the
//! same stem (the ambiguity is counted, never hidden).

use super::{
    line_at, CorpusFile, CorpusIndex, DetectorCounters, EdgeDetector, EdgeDraft, SectionCtx,
};
use std::sync::atomic::{AtomicU64, Ordering};

pub const TAG: &str = "wikilink@1";
pub const EDGE_TYPE: &str = "wikilink";
pub const WEIGHT: f32 = 1.0;
pub const CONFIDENCE: f32 = 0.95;

/// Ignore absurd "targets": an unmatched `[[` in a code block would otherwise
/// swallow the rest of the section into one bogus link.
const MAX_TARGET_LEN: usize = 512;

#[derive(Default)]
pub struct Wikilink {
    unresolved: AtomicU64,
    ambiguous: AtomicU64,
}

enum Resolved<'a> {
    Hit(&'a CorpusFile),
    Ambiguous(&'a CorpusFile),
    Miss,
}

/// §6.5 resolution order: exact rel path (with, then without, extension),
/// else the lowercase stem table. Multiple candidates pick the
/// lexicographically smallest rel — deterministic, and counted as ambiguous.
fn resolve<'a>(corpus: &'a CorpusIndex, target: &str) -> Resolved<'a> {
    if let Some(f) = corpus.files.get(target) {
        return Resolved::Hit(f);
    }
    // Whole-path match minus extension: "notes/beta" → "notes/beta.md".
    // BTreeMap iteration is ascending, so the first match IS the smallest.
    let mut without_ext = corpus
        .files
        .iter()
        .filter(|(rel, _)| super::rel_without_ext(rel) == target);
    if let Some((_, first)) = without_ext.next() {
        return if without_ext.next().is_some() {
            Resolved::Ambiguous(first)
        } else {
            Resolved::Hit(first)
        };
    }
    match corpus.by_stem.get(&target.to_ascii_lowercase()) {
        Some(rels) if rels.len() == 1 => Resolved::Hit(&corpus.files[&rels[0]]),
        Some(rels) => Resolved::Ambiguous(&corpus.files[&rels[0]]),
        None => Resolved::Miss,
    }
}

impl EdgeDetector for Wikilink {
    fn tag(&self) -> &'static str {
        TAG
    }

    fn detect_text(&self, ctx: &SectionCtx<'_>, out: &mut Vec<EdgeDraft>) {
        let text = ctx.text;
        let mut i = 0usize;
        while let Some(open) = text[i..].find("[[") {
            let open = i + open;
            let Some(close) = text[open + 2..].find("]]") else {
                break;
            };
            let close = open + 2 + close;
            i = close + 2;
            let inner = &text[open + 2..close];
            if inner.len() > MAX_TARGET_LEN || inner.contains("[[") {
                continue;
            }
            let target = inner.split('|').next().unwrap_or("").trim();
            if target.is_empty() {
                continue;
            }
            let file = match resolve(ctx.corpus, target) {
                Resolved::Hit(f) => f,
                Resolved::Ambiguous(f) => {
                    self.ambiguous.fetch_add(1, Ordering::Relaxed);
                    f
                }
                Resolved::Miss => {
                    // Dangling link: recorded in the run summary
                    // (edges_unresolved), never an invented edge.
                    self.unresolved.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
            };
            out.push(EdgeDraft {
                src: ctx.section_doc_id.to_string(),
                dst: file.anchor_doc_id.clone(),
                edge_type: EDGE_TYPE,
                weight: WEIGHT,
                confidence: CONFIDENCE,
                valid_at_ms: ctx.file.mtime_ms,
                src_file: ctx.file.rel.clone(),
                quote: line_at(text, open),
                offset: open as u64,
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
            corpus_file("alpha.md", "ka", "notes", "txt-prose", 1753600000000),
            corpus_file("beta.md", "kb", "notes", "txt-prose", 1753600000000),
            corpus_file("sub/beta.txt", "kc", "notes", "txt-prose", 1753600000000),
        ])
    }

    fn run(corpus: &CorpusIndex, text: &str) -> (Vec<EdgeDraft>, DetectorCounters) {
        let det = Wikilink::default();
        let file = &corpus.files["alpha.md"];
        let ctx = SectionCtx {
            corpus,
            file,
            section_ordinal: 0,
            section_doc_id: "sec0",
            text,
        };
        let mut out = Vec::new();
        det.detect_text(&ctx, &mut out);
        (out, det.counters())
    }

    #[test]
    fn fixture_offsets_and_targets() {
        let corpus = corpus();
        let (out, c) = run(
            &corpus,
            "Alpha is the hub note. It links to [[beta]] and [[gamma]].",
        );
        // §6.5 resolution order: `[[beta]]` is an exact rel-path match minus
        // extension (beta.md) — ONE candidate, so it is NOT ambiguous; the
        // stem table (where sub/beta.txt would compete) is never consulted.
        // gamma is dangling and must be counted, not emitted.
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].dst, corpus.files["beta.md"].anchor_doc_id);
        assert_eq!(out[0].offset, 35);
        assert_eq!(
            out[0].quote,
            "Alpha is the hub note. It links to [[beta]] and [[gamma]]."
        );
        assert_eq!(c.unresolved, 1);
        assert_eq!(
            c.ambiguous, 0,
            "exact rel-without-extension wins outright (ambiguity is a stem-table concept)"
        );
    }

    #[test]
    fn alias_exact_rel_and_case_insensitive_stem() {
        let corpus = corpus();
        let (out, c) = run(&corpus, "see [[beta.md|the beta note]] and [[BETA]]");
        assert_eq!(out.len(), 2);
        // exact rel match is unambiguous even though the stem table is not
        assert_eq!(out[0].dst, corpus.files["beta.md"].anchor_doc_id);
        assert_eq!(c.ambiguous, 1, "only the stem lookup was ambiguous");
        assert_eq!(c.unresolved, 0);
    }

    #[test]
    fn ambiguity_is_deterministic_smallest_rel() {
        let corpus = corpus();
        let (a, _) = run(&corpus, "[[beta]]");
        let (b, _) = run(&corpus, "[[beta]]");
        assert_eq!(a[0].dst, b[0].dst);
        assert_eq!(a[0].dst, corpus.files["beta.md"].anchor_doc_id);
    }

    #[test]
    fn unterminated_and_empty_links_emit_nothing() {
        let corpus = corpus();
        let (out, c) = run(&corpus, "broken [[beta and empty [[]] end");
        assert!(out.is_empty());
        assert_eq!(c.unresolved, 0);
    }
}
