//! samedir@1 — files sharing a directory, chained, never a clique.
//!
//! Directory co-location is weak evidence (weight 0.3, confidence 0.4 — the
//! floor of the detector set): it often means "same topic", but it is the
//! only detector that fires on a corpus with no links at all, so it is what
//! keeps an unlinked folder from being a graph of isolated points.
//!
//! ## The fan-out cap: a CHAIN over rel-sorted files
//! A clique is O(n²): one 500-file directory would emit 500·499/2 = 124,750
//! edges and drown every explicit-link signal under same_dir noise. The chain
//! emits exactly n-1 edges (499 for that directory) — O(n) — while directory
//! cohesion stays reachable inside the engine's 2-hop expansion cap for
//! small dirs (±2 rel-sorted neighbors per hop pair). Sorting by rel makes
//! the chain deterministic AND naturally follows name-ordered conventions
//! (01-intro, 02-setup, …), which is why this detector doubles as the
//! adjacent-file "sequence by naming" signal at the file level.

use super::{CorpusIndex, EdgeDetector, EdgeDraft};

pub const TAG: &str = "samedir@1";
pub const EDGE_TYPE: &str = "same_dir";
pub const WEIGHT: f32 = 0.3;
pub const CONFIDENCE: f32 = 0.4;

pub struct SameDir;

impl EdgeDetector for SameDir {
    fn tag(&self) -> &'static str {
        TAG
    }

    fn detect_structure(&self, corpus: &CorpusIndex, out: &mut Vec<EdgeDraft>) {
        // corpus.files is a BTreeMap keyed by rel: iteration is globally
        // rel-sorted, so per-directory order needs no extra sort.
        let mut by_dir: std::collections::BTreeMap<&str, Vec<&super::CorpusFile>> =
            std::collections::BTreeMap::new();
        for f in corpus.files.values() {
            by_dir.entry(f.dir.as_str()).or_default().push(f);
        }
        for (dir, files) in by_dir {
            let shown_dir = if dir.is_empty() { "." } else { dir };
            for pair in files.windows(2) {
                let (left, right) = (pair[0], pair[1]);
                out.push(EdgeDraft {
                    src: left.anchor_doc_id.clone(),
                    dst: right.anchor_doc_id.clone(),
                    edge_type: EDGE_TYPE,
                    weight: WEIGHT,
                    confidence: CONFIDENCE,
                    valid_at_ms: left.mtime_ms,
                    src_file: left.rel.clone(),
                    quote: format!("{} and {} share directory {shown_dir}", left.rel, right.rel),
                    offset: 0,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{corpus_file, CorpusIndex};
    use super::*;

    fn chain(rels: &[&str]) -> Vec<EdgeDraft> {
        let corpus = CorpusIndex::build(
            rels.iter()
                .enumerate()
                .map(|(i, rel)| {
                    corpus_file(rel, &format!("k{i}"), "notes", "txt-prose", 100 + i as i64)
                })
                .collect(),
        );
        let mut out = Vec::new();
        SameDir.detect_structure(&corpus, &mut out);
        out
    }

    /// The §8 fixture directory: 5 files chain into exactly 4 edges in
    /// rel-sorted order — never the 10-edge clique.
    #[test]
    fn fixture_folder_chains_not_cliques() {
        let out = chain(&["alpha.md", "beta.md", "delta.md", "epsilon.md", "gamma.md"]);
        assert_eq!(out.len(), 4);
        let pairs: Vec<(&str, &str)> = out
            .iter()
            .map(|e| (e.src_file.as_str(), e.quote.as_str()))
            .collect();
        assert_eq!(
            pairs,
            vec![
                ("alpha.md", "alpha.md and beta.md share directory ."),
                ("beta.md", "beta.md and delta.md share directory ."),
                ("delta.md", "delta.md and epsilon.md share directory ."),
                ("epsilon.md", "epsilon.md and gamma.md share directory ."),
            ]
        );
    }

    /// The cap in numbers: n files emit exactly n-1 edges, so a 500-file
    /// directory emits 499 — not 124,750.
    #[test]
    fn fan_out_is_linear_in_directory_size() {
        let rels: Vec<String> = (0..500).map(|i| format!("dir/f{i:03}.md")).collect();
        let refs: Vec<&str> = rels.iter().map(String::as_str).collect();
        assert_eq!(chain(&refs).len(), 499);
    }

    #[test]
    fn directories_never_cross_and_singletons_emit_nothing() {
        let out = chain(&["a/x.md", "a/y.md", "b/z.md", "lone.md"]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].quote, "a/x.md and a/y.md share directory a");
    }

    /// Numeric-prefix conventions (01-, 02-, …) sort into their intended
    /// reading order, so the chain is also the file-level sequence signal.
    #[test]
    fn numeric_prefixes_chain_in_reading_order() {
        let out = chain(&["c/02-setup.md", "c/01-intro.md", "c/03-usage.md"]);
        assert_eq!(out.len(), 2);
        assert!(out[0].quote.starts_with("c/01-intro.md and c/02-setup.md"));
        assert!(out[1].quote.starts_with("c/02-setup.md and c/03-usage.md"));
    }
}
