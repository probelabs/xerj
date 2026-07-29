//! sequence@1 — section s_{i-1} → s_i within one file.
//!
//! The natural ordering edge: autoindex splits long documents into
//! retrieval-sized sections, and without these edges the split would DESTROY
//! the document's own narrative adjacency (a hop from "the answer" to "the
//! paragraph after it" is exactly what an agent wants when a section ends
//! mid-thought). Weight 0.8 — structural but almost certainly meaningful;
//! confidence 0.99 because the extractor's own ordering cannot be wrong about
//! itself.
//!
//! Emitted from `detect_text` on section i>0 (the contract's trick to avoid a
//! second pass): every section after the first knows its predecessor's doc id
//! deterministically from (slug, file_key, "s{i-1}").

use super::{EdgeDetector, EdgeDraft, SectionCtx};

pub const TAG: &str = "sequence@1";
pub const EDGE_TYPE: &str = "sequence";
pub const WEIGHT: f32 = 0.8;
pub const CONFIDENCE: f32 = 0.99;

pub struct Sequence;

impl EdgeDetector for Sequence {
    fn tag(&self) -> &'static str {
        TAG
    }

    fn detect_text(&self, ctx: &SectionCtx<'_>, out: &mut Vec<EdgeDraft>) {
        if ctx.section_ordinal == 0 {
            return;
        }
        let prev_ordinal = ctx.section_ordinal - 1;
        let prev_doc_id = crate::ids::doc_id(
            &ctx.file.dataset_slug,
            &ctx.file.file_key,
            &format!("s{prev_ordinal}"),
        );
        out.push(EdgeDraft {
            src: prev_doc_id,
            dst: ctx.section_doc_id.to_string(),
            edge_type: EDGE_TYPE,
            weight: WEIGHT,
            confidence: CONFIDENCE,
            valid_at_ms: ctx.file.mtime_ms,
            src_file: ctx.file.rel.clone(),
            quote: format!(
                "section {prev_ordinal} precedes section {} of {}",
                ctx.section_ordinal, ctx.file.rel
            ),
            offset: 0,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::super::{corpus_file, CorpusIndex, SectionCtx};
    use super::*;

    /// A 3-section file yields exactly the two adjacency edges, chained
    /// through the deterministic section doc ids.
    #[test]
    fn three_sections_chain_two_edges() {
        let corpus = CorpusIndex::build(vec![corpus_file("big.md", "kb", "docs", "txt-prose", 7)]);
        let file = &corpus.files["big.md"];
        let det = Sequence;
        let mut out = Vec::new();
        for ordinal in 0..3u32 {
            let doc_id = crate::ids::doc_id("docs", "kb", &format!("s{ordinal}"));
            let ctx = SectionCtx {
                corpus: &corpus,
                file,
                section_ordinal: ordinal,
                section_doc_id: &doc_id,
                text: "…",
            };
            det.detect_text(&ctx, &mut out);
        }
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].src, crate::ids::doc_id("docs", "kb", "s0"));
        assert_eq!(out[0].dst, crate::ids::doc_id("docs", "kb", "s1"));
        assert_eq!(out[1].src, crate::ids::doc_id("docs", "kb", "s1"));
        assert_eq!(out[1].dst, crate::ids::doc_id("docs", "kb", "s2"));
        assert_eq!(out[0].quote, "section 0 precedes section 1 of big.md");
        assert_eq!(out[0].offset, 0);
        assert_eq!(out[0].valid_at_ms, 7);
    }
}
