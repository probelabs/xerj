//! href@2 — `<a href="…">` to a corpus file, html-extracted files only.
//!
//! Weight 0.7: an HTML anchor is authored intent, but exported HTML (wikis,
//! doc generators) also mass-produces navigation links, so one anchor says
//! less than one hand-written markdown link.
//!
//! @2: dst moved from the target's `s0` section to its file-card node (the
//! raw pass's SRC is the file card too) and edges carry
//! `src_format`/`dst_format`.
//!
//! ## Why there is a raw-source pass
//! The HTML extractor strips markup before sectioning — an `<a>` tag never
//! survives into section text, so `detect_text` alone would make this
//! detector a no-op exactly where it matters. `detect_raw_html` therefore
//! scans the RAW decoded source once per html file (the pipeline feeds it in,
//! §6.6.2): edges anchor to the file's s0 node and carry offset 0, the
//! contract's escape hatch for "the extractor lost byte positions". The two
//! passes cannot double-fire: real tags exist only in the raw source, while
//! entity-escaped tags (`&lt;a …&gt;`) exist only in the decoded section
//! text.

use super::{
    line_at, resolve_local, CorpusFile, CorpusIndex, DetectorCounters, EdgeDetector, EdgeDraft,
    LinkTarget, SectionCtx,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

pub const TAG: &str = "href@2";
pub const EDGE_TYPE: &str = "href";
pub const WEIGHT: f32 = 0.7;
pub const CONFIDENCE: f32 = 0.85;

/// Family string (`Family::as_str`) this detector is scoped to. Sniffing
/// decides — a `.html` extension on a file that sniffed as prose does not
/// qualify, and an extension-less html export does.
const HTML_FAMILY: &str = "html";

#[derive(Default)]
pub struct Href {
    unresolved: AtomicU64,
}

/// Case-insensitive `<a … href=VALUE>`; the capture groups locate the VALUE
/// start byte so section-text hits (escaped markup) carry a real offset.
fn anchor_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r#"(?is)<a\s[^>]*?href\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))"#)
            .expect("static anchor regex")
    })
}

fn scan_anchors(text: &str, mut hit: impl FnMut(usize, &str)) {
    for caps in anchor_re().captures_iter(text) {
        let m = caps.get(1).or_else(|| caps.get(2)).or_else(|| caps.get(3));
        if let Some(m) = m {
            hit(m.start(), m.as_str());
        }
    }
}

fn draft(
    file: &CorpusFile,
    src: &str,
    target: &CorpusFile,
    quote: String,
    offset: u64,
) -> EdgeDraft {
    EdgeDraft {
        src: src.to_string(),
        dst: target.anchor_doc_id.clone(),
        edge_type: EDGE_TYPE,
        weight: WEIGHT,
        confidence: CONFIDENCE,
        valid_at_ms: file.mtime_ms,
        src_file: file.rel.clone(),
        quote,
        offset,
        src_format: file.format.clone(),
        dst_format: target.format.clone(),
    }
}

impl Href {
    /// The raw-source pass (see module docs). `src` is the file's anchor node:
    /// section attribution was lost with the markup, and honesty prefers a
    /// coarse-but-true source node over a guessed section.
    pub fn detect_raw_html(
        &self,
        corpus: &CorpusIndex,
        file: &CorpusFile,
        raw: &str,
        out: &mut Vec<EdgeDraft>,
    ) {
        if file.family != HTML_FAMILY {
            return;
        }
        scan_anchors(raw, |pos, url| {
            match resolve_local(corpus, &file.dir, url) {
                LinkTarget::Hit(target) => out.push(draft(
                    file,
                    &file.anchor_doc_id,
                    target,
                    line_at(raw, pos),
                    // Byte positions did not survive extraction — the §6.5
                    // "best effort" zero, NOT the raw-file offset, which no
                    // reader could map back onto a section.
                    0,
                )),
                LinkTarget::External | LinkTarget::Empty => {}
                LinkTarget::Miss => {
                    self.unresolved.fetch_add(1, Ordering::Relaxed);
                }
            }
        });
    }
}

impl EdgeDetector for Href {
    fn tag(&self) -> &'static str {
        TAG
    }

    /// Section-text pass: only fires when markup survived into the extracted
    /// text (entity-escaped anchors) — then the byte offset is real.
    fn detect_text(&self, ctx: &SectionCtx<'_>, out: &mut Vec<EdgeDraft>) {
        if ctx.file.family != HTML_FAMILY {
            return;
        }
        scan_anchors(ctx.text, |pos, url| {
            match resolve_local(ctx.corpus, &ctx.file.dir, url) {
                LinkTarget::Hit(target) => out.push(draft(
                    ctx.file,
                    ctx.section_doc_id,
                    target,
                    line_at(ctx.text, pos),
                    pos as u64,
                )),
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
            corpus_file("site/index.html", "k1", "d", "html", 10),
            corpus_file("site/about.html", "k2", "d", "html", 10),
            corpus_file("notes.md", "k3", "d", "txt-prose", 10),
        ])
    }

    #[test]
    fn raw_pass_resolves_relative_and_counts_misses() {
        let corpus = corpus();
        let det = Href::default();
        let mut out = Vec::new();
        det.detect_raw_html(
            &corpus,
            &corpus.files["site/index.html"],
            "<p><A HREF='about.html'>about</A> <a class=\"x\" href=\"https://e.co\">ext</a>\n\
             <a href=gone.html>miss</a></p>",
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].dst, corpus.files["site/about.html"].anchor_doc_id);
        assert_eq!(out[0].src, corpus.files["site/index.html"].anchor_doc_id);
        assert_eq!(out[0].offset, 0, "raw pass lost byte positions by contract");
        assert!(out[0].quote.contains("about"));
        assert_eq!(det.counters().unresolved, 1);
    }

    #[test]
    fn non_html_families_are_out_of_scope() {
        let corpus = corpus();
        let det = Href::default();
        let mut out = Vec::new();
        det.detect_raw_html(
            &corpus,
            &corpus.files["notes.md"],
            "<a href=\"site/about.html\">x</a>",
            &mut out,
        );
        let ctx = SectionCtx {
            corpus: &corpus,
            file: &corpus.files["notes.md"],
            section_label: "section 0",
            prev_section: None,
            section_doc_id: "sec0",
            text: "<a href=\"site/about.html\">x</a>",
        };
        det.detect_text(&ctx, &mut out);
        assert!(out.is_empty());
        assert_eq!(det.counters().unresolved, 0);
    }

    #[test]
    fn section_text_pass_keeps_real_offsets() {
        let corpus = corpus();
        let det = Href::default();
        let text = "escaped <a href=\"about.html\">about</a> anchor";
        let ctx = SectionCtx {
            corpus: &corpus,
            file: &corpus.files["site/index.html"],
            section_label: "section 0",
            prev_section: None,
            section_doc_id: "sec0",
            text,
        };
        let mut out = Vec::new();
        det.detect_text(&ctx, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].src, "sec0");
        assert_eq!(out[0].offset, 17, "offset of the href VALUE start");
        assert_eq!(&text[17..27], "about.html");
    }
}
