//! sequence@2 — the file card opens the chain, then each section precedes
//! the next within one file.
//!
//! The natural ordering edge: autoindex splits long documents into
//! retrieval-sized sections, and without these edges the split would DESTROY
//! the document's own narrative adjacency (a hop from "the answer" to "the
//! paragraph after it" is exactly what an agent wants when a section ends
//! mid-thought). Weight 0.8 — structural but almost certainly meaningful;
//! confidence 0.99 because the pipeline's own staging order cannot be wrong
//! about itself.
//!
//! ## @2 — what changed and why
//! - **The chain starts at the file card** (`CorpusFile::anchor_doc_id`):
//!   the first section of every text-family file gets a
//!   `card → first section` edge. Without it the card — where every incoming
//!   citation lands — would float disconnected from the document's own
//!   content, and a 2-hop look around a cited file could never reach what the
//!   file says.
//! - **Predecessor comes from the pipeline's stream order**
//!   (`SectionCtx::prev_section`), not from arithmetic on the locator. That
//!   is what lets PDF sections (`p{page}-s{sec}` locators) chain across page
//!   boundaries: `p2-s0`'s predecessor is the LAST section of page 1, which
//!   no locator computation can name. `s{i}` files chain exactly as @1 did.

use super::{EdgeDetector, EdgeDraft, SectionCtx};

pub const TAG: &str = "sequence@2";
pub const EDGE_TYPE: &str = "sequence";
pub const WEIGHT: f32 = 0.8;
pub const CONFIDENCE: f32 = 0.99;

pub struct Sequence;

impl EdgeDetector for Sequence {
    fn tag(&self) -> &'static str {
        TAG
    }

    fn detect_text(&self, ctx: &SectionCtx<'_>, out: &mut Vec<EdgeDraft>) {
        let (src, quote) = match ctx.prev_section {
            Some((prev_id, prev_label)) => (
                prev_id.to_string(),
                format!(
                    "{prev_label} precedes {} of {}",
                    ctx.section_label, ctx.file.rel
                ),
            ),
            None => (
                ctx.file.anchor_doc_id.clone(),
                format!("{} opens {}", ctx.section_label, ctx.file.rel),
            ),
        };
        out.push(EdgeDraft {
            src,
            dst: ctx.section_doc_id.to_string(),
            edge_type: EDGE_TYPE,
            weight: WEIGHT,
            confidence: CONFIDENCE,
            valid_at_ms: ctx.file.mtime_ms,
            src_file: ctx.file.rel.clone(),
            quote,
            offset: 0,
            src_format: ctx.file.format.clone(),
            dst_format: ctx.file.format.clone(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::super::{corpus_file, CorpusIndex, SectionCtx};
    use super::*;

    /// A 3-section file yields the opening card→s0 edge plus the two
    /// adjacency edges, chained through the deterministic section doc ids.
    #[test]
    fn three_sections_chain_from_the_file_card() {
        let corpus = CorpusIndex::build(vec![corpus_file("big.md", "kb", "docs", "txt-prose", 7)]);
        let file = &corpus.files["big.md"];
        let det = Sequence;
        let mut out = Vec::new();
        let ids: Vec<String> = (0..3u32)
            .map(|i| crate::ids::doc_id("docs", "kb", &format!("s{i}")))
            .collect();
        let labels: Vec<String> = (0..3u32).map(|i| format!("section {i}")).collect();
        let mut prev: Option<(usize, usize)> = None;
        for ordinal in 0..3usize {
            let ctx = SectionCtx {
                corpus: &corpus,
                file,
                section_label: &labels[ordinal],
                prev_section: prev.map(|(i, l)| (ids[i].as_str(), labels[l].as_str())),
                section_doc_id: &ids[ordinal],
                text: "…",
            };
            det.detect_text(&ctx, &mut out);
            prev = Some((ordinal, ordinal));
        }
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].src, file.anchor_doc_id, "chain opens at the card");
        assert_eq!(out[0].dst, ids[0]);
        assert_eq!(out[0].quote, "section 0 opens big.md");
        assert_eq!(out[1].src, ids[0]);
        assert_eq!(out[1].dst, ids[1]);
        assert_eq!(out[1].quote, "section 0 precedes section 1 of big.md");
        assert_eq!(out[2].quote, "section 1 precedes section 2 of big.md");
        assert_eq!(out[0].offset, 0);
        assert_eq!(out[0].valid_at_ms, 7);
        assert_eq!(out[0].src_format, "md");
        assert_eq!(out[0].dst_format, "md");
    }

    /// PDF page-boundary chaining: with stream-order predecessors, page 2's
    /// first section links back to page 1's LAST section — the edge the
    /// locator arithmetic of @1 could never produce.
    #[test]
    fn pdf_sections_chain_across_page_boundaries() {
        let corpus = CorpusIndex::build(vec![corpus_file("brief.pdf", "kp", "docs", "pdf", 9)]);
        let file = &corpus.files["brief.pdf"];
        let det = Sequence;
        let mut out = Vec::new();
        let p1s1 = crate::ids::doc_id("docs", "kp", "p1-s1");
        let p2s0 = crate::ids::doc_id("docs", "kp", "p2-s0");
        let ctx = SectionCtx {
            corpus: &corpus,
            file,
            section_label: "page 2 section 0",
            prev_section: Some((p1s1.as_str(), "page 1 section 1")),
            section_doc_id: &p2s0,
            text: "…",
        };
        det.detect_text(&ctx, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].src, p1s1);
        assert_eq!(out[0].dst, p2s0);
        assert_eq!(
            out[0].quote,
            "page 1 section 1 precedes page 2 section 0 of brief.pdf"
        );
        assert_eq!(out[0].src_format, "pdf");
    }
}
