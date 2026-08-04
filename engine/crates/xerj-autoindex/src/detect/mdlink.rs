//! mdlink@2 — `[text](relative/path.md)` where the target is a corpus file.
//!
//! Slightly below wikilink (weight 0.9): an inline markdown link is still an
//! explicit authorial act, but it is also how people cite external material,
//! so a resolved local target carries marginally less "these notes belong
//! together" intent than a `[[wiki link]]`.
//!
//! @2: dst moved from the target's `s0` section to its file-card node and
//! edges carry `src_format`/`dst_format`.

use super::{
    line_at, resolve_local, DetectorCounters, EdgeDetector, EdgeDraft, LinkTarget, SectionCtx,
};
use std::sync::atomic::{AtomicU64, Ordering};

pub const TAG: &str = "mdlink@2";
pub const EDGE_TYPE: &str = "mdlink";
pub const WEIGHT: f32 = 0.9;
pub const CONFIDENCE: f32 = 0.9;

#[derive(Default)]
pub struct Mdlink {
    unresolved: AtomicU64,
}

/// Find `[label](url)` occurrences. Hand scan, not regex: the grammar is two
/// delimiter searches, and the scan must never backtrack pathologically on a
/// section full of stray brackets. No nesting support — deterministic first
/// `]` / first `)` wins, which matches how common renderers degrade.
fn scan_links(text: &str, mut hit: impl FnMut(usize, &str)) {
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let Some(open) = text[i..].find('[') else {
            break;
        };
        let open = i + open;
        let Some(close) = text[open + 1..].find(']') else {
            break;
        };
        let close = open + 1 + close;
        if bytes.get(close + 1) != Some(&b'(') {
            i = open + 1;
            continue;
        }
        let Some(paren) = text[close + 2..].find(')') else {
            break;
        };
        let paren = close + 2 + paren;
        // `[t](url "title")` — the optional title starts at the first
        // whitespace inside the parens.
        let raw = text[close + 2..paren].trim();
        let url = raw.split_whitespace().next().unwrap_or("");
        hit(open, url);
        i = paren + 1;
    }
}

impl EdgeDetector for Mdlink {
    fn tag(&self) -> &'static str {
        TAG
    }

    fn detect_text(&self, ctx: &SectionCtx<'_>, out: &mut Vec<EdgeDraft>) {
        scan_links(ctx.text, |open, url| {
            match resolve_local(ctx.corpus, &ctx.file.dir, url) {
                LinkTarget::Hit(target) => out.push(EdgeDraft {
                    src: ctx.section_doc_id.to_string(),
                    dst: target.anchor_doc_id.clone(),
                    edge_type: EDGE_TYPE,
                    weight: WEIGHT,
                    confidence: CONFIDENCE,
                    valid_at_ms: ctx.file.mtime_ms,
                    src_file: ctx.file.rel.clone(),
                    quote: line_at(ctx.text, open),
                    offset: open as u64,
                    src_format: ctx.file.format.clone(),
                    dst_format: target.format.clone(),
                }),
                // External links are what markdown links are FOR — skipping
                // them silently is correct, only local misses are dangling.
                LinkTarget::External | LinkTarget::Empty => {}
                LinkTarget::Miss => {
                    self.unresolved.fetch_add(1, Ordering::Relaxed);
                }
            }
        });
    }

    fn counters(&self) -> DetectorCounters {
        DetectorCounters {
            unresolved: self.unresolved.load(Ordering::Relaxed),
            ambiguous: 0,
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
            corpus_file("docs/guide.md", "k1", "d", "txt-prose", 10),
            corpus_file("readme.md", "k2", "d", "txt-prose", 10),
        ])
    }

    fn run(corpus: &CorpusIndex, from: &str, text: &str) -> (Vec<EdgeDraft>, DetectorCounters) {
        let det = Mdlink::default();
        let file = &corpus.files[from];
        let ctx = SectionCtx {
            corpus,
            file,
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
    fn resolves_dir_relative_then_root_relative() {
        let corpus = corpus();
        let (out, c) = run(
            &corpus,
            "docs/guide.md",
            "see [readme](../readme.md) and [self](guide.md) and [root](docs/guide.md)",
        );
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].dst, corpus.files["readme.md"].anchor_doc_id);
        assert_eq!(out[1].dst, corpus.files["docs/guide.md"].anchor_doc_id);
        // "docs/guide.md" fails dir-relative (docs/docs/guide.md), resolves
        // root-relative.
        assert_eq!(out[2].dst, corpus.files["docs/guide.md"].anchor_doc_id);
        assert_eq!(c.unresolved, 0);
        assert_eq!(out[0].offset, 4);
    }

    #[test]
    fn external_links_skip_and_local_misses_count() {
        let corpus = corpus();
        let (out, c) = run(
            &corpus,
            "readme.md",
            "[web](https://example.com/a.md) [mail](mailto:x@y.z) [gone](missing.md) [frag](#top)",
        );
        assert!(out.is_empty());
        assert_eq!(c.unresolved, 1, "only the local miss is dangling");
    }

    #[test]
    fn title_and_fragment_are_stripped() {
        let corpus = corpus();
        let (out, _) = run(
            &corpus,
            "readme.md",
            "x [g](docs/guide.md#section \"The Guide\") y",
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].dst, corpus.files["docs/guide.md"].anchor_doc_id);
        assert_eq!(out[0].quote, "x [g](docs/guide.md#section \"The Guide\") y");
    }

    #[test]
    fn wikilinks_do_not_false_positive() {
        let corpus = corpus();
        let (out, c) = run(&corpus, "readme.md", "a [[guide]] wikilink, no mdlink here");
        assert!(out.is_empty());
        assert_eq!(c.unresolved, 0);
    }
}
