//! sharedterm@1 — two documents that use the same distinctive vocabulary.
//!
//! The gap this closes (issue #164): every other detector needs an explicit
//! citation (`wikilink`, `mdlink`, `href`, `pathcite`, `cratecite`) or a
//! filesystem relationship (`sequence`, `samedir`). Point the brain at a folder
//! of PDFs or saved pages — documents that never link to each other — and the
//! map draws groups with almost no lines between them. This detector is the
//! only one that connects two documents on the strength of what they SAY.
//!
//! ## The honest limit, stated first
//! This links documents that share VOCABULARY, not documents that MEAN the
//! same thing. There is no model here: terms are compared as strings, and not
//! even stemmed — "ferment", "ferments" and "fermentation" are three different
//! terms. Two papers that discuss the same idea in different words are not
//! linked, and two documents that share a rare word by coincidence are. That is
//! why the shared terms travel with the edge as evidence — a wrong link is
//! inspectable exactly like a wrong `mdlink`, and the reader decides.
//!
//! ## Weight 0.45 / confidence 0.5 — where this ranks
//! Above `samedir@2` (0.3/0.4): sitting in the same folder says a human filed
//! two documents together, which is often about topic and often about nothing.
//! Sharing distinctive words is direct evidence from the documents themselves.
//! Below every authored link (`pathcite@1` is 0.6/0.7, `wikilink@2` is
//! 1.0/0.95): a human who wrote a citation asserted the relationship; this
//! detector only infers one. Confidence 0.5 says exactly what it means — an
//! inference with real evidence behind it and a real error rate.
//!
//! ## Density: the difference between a map and a hairball
//! Tuned loosely, term overlap links everything to everything. Two named
//! constants, both tunable without archaeology:
//!
//! - [`MAX_DOC_FRACTION`] — a term in more than 10% of the corpus is
//!   vocabulary, not topic ("model" in a folder of ML papers), and is ignored.
//!   [`MAX_TERM_DOCS`] is the absolute ceiling on top of that: pairing a term
//!   costs df²/2 comparisons, and a term shared by dozens of documents cannot
//!   be distinctive however large the corpus is.
//! - [`MAX_EDGES_PER_DOC`] — a hard degree budget: no document ever ends up
//!   with more than 5 `shared_term` edges, in or out. Candidate pairs are
//!   ranked by how many distinctive terms they share and the budget is spent on
//!   the strongest; everything it refuses is counted (`DetectorCounters.capped`)
//!   and reported in the run summary, so what the map did not draw is on the
//!   record rather than silently gone.
//!
//! A consequence of the 10% rule worth stating out loud: a corpus of ten
//! documents or fewer produces NO `shared_term` edges at all, because a term
//! shared by two of ten documents is in 20% of the corpus. That is the rule
//! being honest rather than an exception — below that size "distinctive" has
//! nothing to measure against, and `samedir` already chains a ten-file folder
//! end to end.
//!
//! ## What it did on a real corpus
//! Measured over 240 arXiv PDFs filed in 24 topic folders (236 of them
//! extractable), indexed with the same binary twice:
//!
//! - before: 11,854 edges, of which 11,642 `sequence` (inside one document)
//!   and 212 `same_dir`. Cross-document edges: 212, none of them leaving a
//!   folder. The corpus contains no resolvable citations, so no other detector
//!   fired at all.
//! - after: +462 `shared_term` edges (271 more refused by the cap), 377 of them
//!   joining documents in different folders and 296 joining different top-level
//!   topics — connections no other detector could make.
//! - Both runs produced byte-identical counts and evidence.
//!
//! How good are they? Filed topic is a rough ground truth, so: a random pair of
//! these documents shares a top-level topic 17.0% of the time and a leaf folder
//! 9.7%. `shared_term` links do so 35.9% and 18.4% of the time — about twice
//! chance. Links resting on 3+ shared words reach 41.9% / 23.3%; the 2-word
//! floor keeps the weaker 32.4% / 15.5% band because a cross-topic link is
//! often right (the same benchmark discussed in two areas) and "same folder"
//! undercounts correctness. This is a real signal and a noisy one — which is
//! exactly why every edge carries its words.
//!
//! ## Deterministic
//! Same folder in, same edges out, in the same order — no wall clock, no
//! randomness, no model. Per-file term counts accumulate in stream order inside
//! one worker; every cross-document structure is a `BTreeMap`/`BTreeSet`, so
//! worker interleaving cannot reach the output. Every tie is broken on the term
//! or the rel path, never on iteration order.
//!
//! ## What it costs, and what it does not see
//! Tracked state is bounded by construction: at most
//! [`TRACK_TERMS_PER_DOC`] terms per document (pruned by in-document frequency
//! whenever a document exceeds [`PRUNE_AT`] distinct terms) and at most
//! [`MAX_PARTNERS_TRACKED`] candidate partners per document.
//!
//! Two limits worth knowing:
//! - Only PROSE sections reach a detector's `detect_text` (the pipeline calls
//!   it for `s{i}`/`p{page}-s{sec}` locators), so CSV rows and log lines
//!   contribute no terms and get no `shared_term` edges.
//! - An incremental run only re-extracts the files that changed, so it can only
//!   compare those files with each other. Edges from earlier runs stay live and
//!   nothing is lost, but a document added today is compared against the whole
//!   corpus only on a run that re-reads the whole corpus (`--fresh`).

use super::{clip_quote, CorpusIndex, DetectorCounters, EdgeDetector, EdgeDraft, SectionCtx};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

pub const TAG: &str = "sharedterm@1";
pub const EDGE_TYPE: &str = "shared_term";
pub const WEIGHT: f32 = 0.45;
pub const CONFIDENCE: f32 = 0.5;

/// Hard degree budget per document. 5 keeps a 240-document corpus under 600
/// `shared_term` edges — a map a person can read — while still giving every
/// document room for its topic neighbourhood. Raise it and the map gets denser,
/// not more informative; the cap is the whole difference between a map and a
/// hairball.
pub const MAX_EDGES_PER_DOC: usize = 5;

/// Distinctiveness floor: a term appearing in more than this fraction of the
/// corpus is the corpus's own vocabulary, not a topic. 10% is deliberately
/// strict — in a folder of 240 machine-learning papers "model" and "training"
/// are in most of them and connect nothing.
pub const MAX_DOC_FRACTION: f64 = 0.10;

/// Absolute ceiling on document frequency, whatever 10% works out to. Pairing a
/// term costs df·(df-1)/2 comparisons, and a word shared by more than 64
/// documents is not the reason any two of them are related.
pub const MAX_TERM_DOCS: usize = 64;

/// Distinctive terms kept per document for linking, ranked by how often THIS
/// document uses them. Measured on a 240-paper PDF library, ranking by global
/// rarity instead picked each document's accidents — a stray "rss", an author
/// surname, a token the extractor split — because the rarest word in a PDF is
/// usually its noisiest. The words a document repeats are the words it is
/// about, and 24 of them is a topic.
pub const TOP_TERMS_PER_DOC: usize = 24;

/// A single word in common is a coincidence; two independently distinctive
/// words is the floor for asserting a relationship. Measured on the same
/// library, 405 of 558 candidate links rested on ONE shared term and read as
/// noise, which is what this constant exists to refuse.
///
/// Counted in WORD FAMILIES, not raw terms — see [`families`]. There is no
/// stemming here, so "trajectory" and "trajectories" are two terms and one
/// word, and without the family rule a singular and its plural would satisfy
/// this floor on their own.
pub const MIN_SHARED_TERMS: usize = 2;

/// Shared leading characters that make two terms one word family. Applied to
/// alphabetically adjacent terms, so a term that is a prefix of the next
/// ("bid"/"bids") merges at any length and longer pairs merge on a 5-character
/// stem ("recommender"/"recommendations"). Deliberately crude: it exists to
/// stop one word counting twice, not to be a stemmer.
pub const TERM_FAMILY_PREFIX: usize = 5;

/// Candidate partners tracked per document before ranking — 6× the edge budget,
/// enough headroom to rank properly, and the reason peak memory is
/// O(documents × 32) pairs rather than O(documents²).
pub const MAX_PARTNERS_TRACKED: usize = 32;

/// Terms kept per document after pruning (the ones with the highest
/// in-document counts).
pub const TRACK_TERMS_PER_DOC: usize = 256;

/// Distinct terms a document may accumulate before it is pruned back to
/// [`TRACK_TERMS_PER_DOC`]. Pruning is by in-document frequency, so a long
/// document keeps the words it actually repeats.
pub const PRUNE_AT: usize = 1024;

/// Shortest term considered. 3 keeps real acronyms (rag, rna, gan); 2 would
/// admit mostly noise.
pub const MIN_TERM_LEN: usize = 3;

/// Longest term considered — beyond this it is a hash, a base64 fragment or a
/// PDF extraction artefact, not a word.
pub const MAX_TERM_LEN: usize = 32;

/// Shared terms named in the evidence quote. The quote field is clipped at 240
/// chars, so the edge names the rarest terms first and states the total.
pub const TERMS_IN_EVIDENCE: usize = 12;

/// English function words. Document frequency alone removes these from any
/// corpus large enough for the detector to fire; the list is belt-and-braces
/// for corpora of very short documents, where a function word can be rare
/// enough to slip under the 10% ceiling and become the evidence on an edge.
pub const STOPWORDS: &[&str] = &[
    "about", "after", "all", "also", "and", "any", "are", "because", "been", "before", "being",
    "between", "both", "but", "can", "could", "did", "does", "each", "for", "from", "had", "has",
    "have", "her", "here", "him", "his", "how", "into", "its", "just", "may", "might", "more",
    "most", "must", "not", "now", "only", "other", "our", "out", "over", "own", "same", "she",
    "should", "since", "some", "such", "than", "that", "the", "their", "them", "then", "there",
    "these", "they", "this", "those", "through", "thus", "too", "under", "use", "used", "using",
    "very", "was", "were", "what", "when", "where", "which", "while", "who", "why", "will", "with",
    "would", "you", "your",
];

fn is_stopword(term: &str) -> bool {
    STOPWORDS.binary_search(&term).is_ok()
}

/// Terms of one document, plus the counts that decide what survives pruning.
#[derive(Default)]
struct DocTerms {
    counts: BTreeMap<String, u32>,
}

impl DocTerms {
    /// Keep the [`TRACK_TERMS_PER_DOC`] most-repeated terms; ties on the term
    /// itself so pruning is deterministic.
    fn prune(&mut self) {
        if self.counts.len() <= PRUNE_AT {
            return;
        }
        let mut ranked: Vec<(&String, &u32)> = self.counts.iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        let keep: BTreeSet<String> = ranked
            .into_iter()
            .take(TRACK_TERMS_PER_DOC)
            .map(|(t, _)| t.clone())
            .collect();
        self.counts.retain(|t, _| keep.contains(t));
    }
}

/// Lowercased alphanumeric runs, minus stopwords, pure numbers and anything
/// outside the length window. Unicode letters are terms too — a corpus is not
/// obliged to be English.
fn count_terms(text: &str, into: &mut BTreeMap<String, u32>) {
    let mut term = String::new();
    let flush = |term: &mut String, into: &mut BTreeMap<String, u32>| {
        if term.is_empty() {
            return;
        }
        let keep = (MIN_TERM_LEN..=MAX_TERM_LEN).contains(&term.chars().count())
            && term.chars().any(char::is_alphabetic)
            && !is_stopword(term.as_str());
        if keep {
            *into.entry(std::mem::take(term)).or_default() += 1;
        } else {
            term.clear();
        }
    };
    for c in text.chars() {
        if c.is_alphanumeric() {
            for lower in c.to_lowercase() {
                term.push(lower);
            }
        } else {
            flush(&mut term, into);
        }
    }
    flush(&mut term, into);
}

#[derive(Default)]
pub struct SharedTerm {
    /// rel path → terms of that document. One `BTreeMap` so the corpus pass
    /// sees documents in rel order regardless of which worker read them.
    docs: Mutex<BTreeMap<String, DocTerms>>,
    /// Candidate pairs the per-document budget refused.
    capped: AtomicU64,
}

impl EdgeDetector for SharedTerm {
    fn tag(&self) -> &'static str {
        TAG
    }

    /// Accumulate only — an edge cannot be judged until every document's terms
    /// are known, because "distinctive" is a fact about the whole corpus.
    fn detect_text(&self, ctx: &SectionCtx<'_>, _out: &mut Vec<EdgeDraft>) {
        let mut counts: BTreeMap<String, u32> = BTreeMap::new();
        count_terms(ctx.text, &mut counts);
        if counts.is_empty() {
            return;
        }
        let mut docs = self.docs.lock().unwrap_or_else(|e| e.into_inner());
        let entry = docs.entry(ctx.file.rel.clone()).or_default();
        for (term, n) in counts {
            *entry.counts.entry(term).or_default() += n;
        }
        entry.prune();
    }

    fn detect_corpus(&self, corpus: &CorpusIndex, out: &mut Vec<EdgeDraft>) {
        let docs = std::mem::take(&mut *self.docs.lock().unwrap_or_else(|e| e.into_inner()));
        // Only documents that are still in the corpus table can carry an edge.
        let docs: BTreeMap<&str, &DocTerms> = docs
            .iter()
            .filter(|(rel, _)| corpus.files.contains_key(rel.as_str()))
            .map(|(rel, t)| (rel.as_str(), t))
            .collect();
        if docs.len() < 2 {
            return;
        }

        // 1. Document frequency over everything tracked, and the ceiling a term
        //    must stay under to count as distinctive.
        let mut df: BTreeMap<&str, usize> = BTreeMap::new();
        for terms in docs.values() {
            for term in terms.counts.keys() {
                *df.entry(term.as_str()).or_default() += 1;
            }
        }
        let max_term_docs =
            ((docs.len() as f64 * MAX_DOC_FRACTION).ceil() as usize).min(MAX_TERM_DOCS);
        if max_term_docs < 2 {
            // Ten documents or fewer: a term in two of them is in 20% of the
            // corpus. Nothing here is distinctive, so nothing is asserted.
            return;
        }

        // 2. Each document's own distinctive vocabulary: most-repeated first
        //    (the words it is ABOUT), then rarest across the corpus, then the
        //    term itself — a total order, so the selection is the same on every
        //    run. See TOP_TERMS_PER_DOC for why frequency leads and rarity does
        //    not.
        let mut selected: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (rel, terms) in &docs {
            let mut ranked: Vec<(std::cmp::Reverse<u32>, usize, &str)> = terms
                .counts
                .iter()
                .filter_map(|(term, count)| {
                    let d = *df.get(term.as_str())?;
                    (2..=max_term_docs).contains(&d).then_some((
                        std::cmp::Reverse(*count),
                        d,
                        term.as_str(),
                    ))
                })
                .collect();
            ranked.sort_unstable();
            ranked.truncate(TOP_TERMS_PER_DOC);
            if !ranked.is_empty() {
                selected.insert(rel, ranked.into_iter().map(|(_, _, t)| t).collect());
            }
        }

        // 3. Postings over the SELECTED terms only, then pairs. Terms are
        //    walked rarest-first so that when a document hits its
        //    MAX_PARTNERS_TRACKED headroom, the partners it kept are the ones
        //    that shared its rarest words.
        let mut postings: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (rel, terms) in &selected {
            for term in terms {
                postings.entry(term).or_default().push(rel);
            }
        }
        let mut by_rarity: Vec<(usize, &str)> = postings
            .iter()
            .filter(|(_, rels)| rels.len() >= 2)
            .map(|(term, rels)| (rels.len(), *term))
            .collect();
        by_rarity.sort_unstable();

        let mut pairs: BTreeMap<(&str, &str), Vec<&str>> = BTreeMap::new();
        let mut partners: BTreeMap<&str, usize> = BTreeMap::new();
        for (_, term) in by_rarity {
            let rels = &postings[term];
            for (i, a) in rels.iter().enumerate() {
                for b in &rels[i + 1..] {
                    let key = if a < b { (*a, *b) } else { (*b, *a) };
                    match pairs.get_mut(&key) {
                        Some(shared) => shared.push(term),
                        None => {
                            let a_full = partners.get(key.0).copied().unwrap_or(0);
                            let b_full = partners.get(key.1).copied().unwrap_or(0);
                            if a_full >= MAX_PARTNERS_TRACKED || b_full >= MAX_PARTNERS_TRACKED {
                                continue;
                            }
                            *partners.entry(key.0).or_default() += 1;
                            *partners.entry(key.1).or_default() += 1;
                            pairs.insert(key, vec![term]);
                        }
                    }
                }
            }
        }

        // 4. One shared word is a coincidence — refuse it before anything is
        //    ranked, so the budget is spent on relationships rather than on
        //    accidents, and `capped` counts only candidates that were real.
        //    Counted in word families, so a plural cannot be the second word.
        pairs.retain(|_, shared| families(shared) >= MIN_SHARED_TERMS);

        // 5. Spend the per-document budget on the strongest pairs. Each
        //    document offers its candidates in (most shared terms, then rel)
        //    order and takes ONE per round, so a document early in rel order
        //    cannot spend the whole neighbourhood's budget before a later one
        //    gets its first link — the isolated note is the failure this
        //    detector exists to fix. A pair is accepted only if BOTH endpoints
        //    still have room, which is what makes the cap a guarantee rather
        //    than an average. Cursors only ever move forward: a full partner
        //    never empties again and an accepted pair never un-accepts.
        let mut candidates: BTreeMap<&str, Vec<(std::cmp::Reverse<usize>, &str)>> = BTreeMap::new();
        for ((a, b), shared) in &pairs {
            let n = shared.len();
            candidates
                .entry(a)
                .or_default()
                .push((std::cmp::Reverse(n), b));
            candidates
                .entry(b)
                .or_default()
                .push((std::cmp::Reverse(n), a));
        }
        for ranked in candidates.values_mut() {
            ranked.sort_unstable();
        }
        let mut degree: BTreeMap<&str, usize> = BTreeMap::new();
        let mut cursor: BTreeMap<&str, usize> = BTreeMap::new();
        let mut accepted: BTreeSet<(&str, &str)> = BTreeSet::new();
        for _round in 0..MAX_EDGES_PER_DOC {
            let mut progressed = false;
            for (rel, ranked) in &candidates {
                if degree.get(rel).copied().unwrap_or(0) >= MAX_EDGES_PER_DOC {
                    continue;
                }
                let at = cursor.entry(*rel).or_default();
                while *at < ranked.len() {
                    let partner = ranked[*at].1;
                    *at += 1;
                    let key = if *rel < partner {
                        (*rel, partner)
                    } else {
                        (partner, *rel)
                    };
                    if accepted.contains(&key) {
                        continue; // already paid for from the other endpoint
                    }
                    if degree.get(partner).copied().unwrap_or(0) >= MAX_EDGES_PER_DOC {
                        continue;
                    }
                    *degree.entry(*rel).or_default() += 1;
                    *degree.entry(partner).or_default() += 1;
                    accepted.insert(key);
                    progressed = true;
                    break;
                }
            }
            if !progressed {
                break;
            }
        }
        self.capped
            .fetch_add((pairs.len() - accepted.len()) as u64, Ordering::Relaxed);

        // 6. Emit. Endpoints are file cards (like samedir): the relationship is
        //    between the documents, not between two particular sections.
        for (a, b) in accepted {
            let (Some(src), Some(dst)) = (corpus.files.get(a), corpus.files.get(b)) else {
                continue;
            };
            let shared = &pairs[&(a, b)];
            out.push(EdgeDraft {
                src: src.anchor_doc_id.clone(),
                dst: dst.anchor_doc_id.clone(),
                edge_type: EDGE_TYPE,
                weight: WEIGHT,
                confidence: CONFIDENCE,
                valid_at_ms: src.mtime_ms,
                src_file: src.rel.clone(),
                quote: evidence(&src.rel, &dst.rel, shared),
                offset: 0,
                src_format: src.format.clone(),
                dst_format: dst.format.clone(),
            });
        }
    }

    fn counters(&self) -> DetectorCounters {
        DetectorCounters {
            capped: self.capped.load(Ordering::Relaxed),
            ..DetectorCounters::default()
        }
    }
}

/// Distinct word families among shared terms: sort, then fold each term into
/// the previous one when it merely extends it or shares its first
/// [`TERM_FAMILY_PREFIX`] characters. Deterministic (sorted input, no state)
/// and total (every term lands in exactly one family).
fn families(shared: &[&str]) -> usize {
    let mut sorted: Vec<&str> = shared.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut n = 0usize;
    let mut head: Option<&str> = None;
    for term in sorted {
        let same = head.is_some_and(|h| {
            term.starts_with(h)
                || h.chars()
                    .zip(term.chars())
                    .take(TERM_FAMILY_PREFIX)
                    .filter(|(a, b)| a == b)
                    .count()
                    >= TERM_FAMILY_PREFIX
        });
        if !same {
            n += 1;
            head = Some(term);
        }
    }
    n
}

/// The evidence line: which terms taught this edge, rarest first, then the two
/// documents. Terms come FIRST deliberately — the quote is clipped at 240
/// chars, and two real corpus paths (an arXiv-style filename runs past 90)
/// would push the evidence off the end of its own evidence field. Measured on
/// the PDF library, 187 of 558 quotes were clipped mid-list before this order.
/// Both full paths remain recoverable: `src_file` carries one and the dst node
/// id resolves to the other.
fn evidence(src: &str, dst: &str, shared: &[&str]) -> String {
    let mut named: Vec<&str> = shared.to_vec();
    named.dedup();
    let total = named.len();
    named.truncate(TERMS_IN_EVIDENCE);
    let plural = if total == 1 { "term" } else { "terms" };
    clip_quote(&format!(
        "{total} distinctive {plural} in common: {} — {src} and {dst}",
        named.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::super::{corpus_file, CorpusFile, CorpusIndex, SectionCtx};
    use super::*;

    /// Feed a whole corpus through the detector: `docs` is (rel, body).
    fn run(docs: &[(&str, &str)]) -> (Vec<EdgeDraft>, DetectorCounters) {
        let files: Vec<CorpusFile> = docs
            .iter()
            .enumerate()
            .map(|(i, (rel, _))| corpus_file(rel, &format!("k{i}"), "notes", "txt-prose", 100))
            .collect();
        let corpus = CorpusIndex::build(files);
        let det = SharedTerm::default();
        let mut out = Vec::new();
        for (rel, body) in docs {
            let section = format!("sec-{rel}");
            let ctx = SectionCtx {
                corpus: &corpus,
                file: &corpus.files[*rel],
                section_label: "section 0",
                prev_section: None,
                section_doc_id: &section,
                text: body,
            };
            det.detect_text(&ctx, &mut out);
        }
        assert!(out.is_empty(), "detect_text emits nothing on its own");
        det.detect_corpus(&corpus, &mut out);
        (out, det.counters())
    }

    /// Filler documents so document frequency has something to measure: each
    /// carries the same everyday words and one word of its own.
    fn filler(n: usize) -> Vec<(String, String)> {
        (0..n)
            .map(|i| {
                (
                    format!("filler{i:02}.md"),
                    format!("The data and the notes were filed. Item unique{i:02} was recorded."),
                )
            })
            .collect()
    }

    fn with_filler(docs: &[(&str, &str)], n: usize) -> (Vec<EdgeDraft>, DetectorCounters) {
        let fill = filler(n);
        let mut all: Vec<(&str, &str)> = docs.to_vec();
        all.extend(fill.iter().map(|(r, b)| (r.as_str(), b.as_str())));
        run(&all)
    }

    /// The point of the detector: two documents that never cite each other are
    /// linked by the words only they use, and the edge says which words.
    #[test]
    fn documents_sharing_distinctive_terms_are_linked_with_the_terms_as_evidence() {
        let (out, _) = with_filler(
            &[
                (
                    "sourdough.md",
                    "The starter ferments overnight. Lactobacillus and wild yeast \
                     do the work; the data on hydration is in the notes.",
                ),
                (
                    "kimchi.md",
                    "Cabbage ferments in brine. Lactobacillus again, and the data \
                     says three days.",
                ),
            ],
            10,
        );
        let edge: Vec<&EdgeDraft> = out
            .iter()
            .filter(|e| e.src_file == "kimchi.md" || e.src_file == "sourdough.md")
            .collect();
        assert_eq!(edge.len(), 1, "exactly one link between the two recipes");
        let e = edge[0];
        assert_eq!(e.edge_type, EDGE_TYPE);
        assert_eq!(e.weight, WEIGHT);
        assert_eq!(e.confidence, CONFIDENCE);
        assert_eq!(e.src_file, "kimchi.md", "src is the rel-smaller endpoint");
        assert!(
            e.quote.contains("lactobacillus") && e.quote.contains("ferments"),
            "the shared terms must travel with the edge: {}",
            e.quote
        );
        assert_eq!(
            e.quote,
            "2 distinctive terms in common: ferments, lactobacillus — kimchi.md and sourdough.md",
            "terms lead the rationale so clipping can never eat the evidence"
        );
        // Everyday words are in every filler document too, so they are not
        // what linked these two.
        assert!(!e.quote.contains("data"), "{}", e.quote);
    }

    /// The other half of the promise: shared everyday vocabulary is not a
    /// relationship, and must not draw a line.
    #[test]
    fn documents_sharing_only_common_words_get_no_edge() {
        let (out, _) = with_filler(
            &[
                ("left.md", "The data and the notes were filed."),
                ("right.md", "The notes and the data were filed."),
            ],
            10,
        );
        assert!(
            out.iter()
                .all(|e| e.src_file != "left.md" && e.src_file != "right.md"),
            "common words linked two unrelated notes: {:?}",
            out.iter().map(|e| e.quote.clone()).collect::<Vec<_>>()
        );
    }

    /// The density cap is a guarantee, not an average: 10 documents that all
    /// use the same distinctive vocabulary are a 45-edge clique, and a clique
    /// is the hairball THE MAP exists to avoid. (90 filler documents put the
    /// clique's terms at exactly the 10% distinctiveness ceiling, so this tests
    /// the cap and not the term filter.)
    #[test]
    fn per_document_cap_is_enforced_and_the_drop_is_counted() {
        let mut docs: Vec<(String, String)> = (0..10)
            .map(|i| {
                (
                    format!("clique{i:02}.md"),
                    "Fermentation, lactobacillus, brine, cabbage, hydration, sourdough."
                        .to_string(),
                )
            })
            .collect();
        docs.extend(filler(90));
        let refs: Vec<(&str, &str)> = docs.iter().map(|(r, b)| (r.as_str(), b.as_str())).collect();
        let (out, counters) = run(&refs);
        let mut degree: BTreeMap<&str, usize> = BTreeMap::new();
        for e in &out {
            let ends = e.quote.rsplit_once(" — ").expect("quote names both ends").1;
            let (a, b) = ends.split_once(" and ").expect("two documents");
            *degree.entry(a).or_default() += 1;
            *degree.entry(b).or_default() += 1;
        }
        assert!(
            degree.values().all(|d| *d <= MAX_EDGES_PER_DOC),
            "a document exceeded the cap: {degree:?}"
        );
        assert!(
            out.len() < 45,
            "the clique must not be drawn: {}",
            out.len()
        );
        assert!(
            degree.len() == 10,
            "the cap must not leave a document isolated: {degree:?}"
        );
        assert!(
            counters.capped > 0,
            "pairs the cap refused must be counted, not silently dropped"
        );
        assert_eq!(
            out.len() as u64 + counters.capped,
            45,
            "every candidate pair is either drawn or counted as capped"
        );
    }

    /// Same input, same edges, same order — the property the whole graph's
    /// identity (and idempotent re-runs) hangs off.
    #[test]
    fn output_is_deterministic_across_runs() {
        let docs = &[
            (
                "a.md",
                "Fermentation and lactobacillus in brine; hydration matters.",
            ),
            ("b.md", "Brine, lactobacillus, cabbage, and fermentation."),
            ("c.md", "Hydration and fermentation of the starter."),
            (
                "d.md",
                "Entirely unrelated: astronomy, telescopes, parallax.",
            ),
            ("e.md", "Astronomy, parallax, and telescopes again."),
        ];
        let (first, _) = with_filler(docs, 10);
        let (second, _) = with_filler(docs, 10);
        let shape = |v: &[EdgeDraft]| -> Vec<(String, String, String)> {
            v.iter()
                .map(|e| (e.src.clone(), e.dst.clone(), e.quote.clone()))
                .collect()
        };
        assert!(!first.is_empty(), "the fixture must produce edges");
        assert_eq!(shape(&first), shape(&second));
    }

    /// Corpus-wide vocabulary is not a topic: a term in more than 10% of the
    /// documents is ignored even though it is not an English stopword.
    #[test]
    fn corpus_wide_jargon_is_not_distinctive() {
        let mut docs: Vec<(String, String)> = (0..20)
            .map(|i| {
                (
                    format!("paper{i:02}.md"),
                    format!("This model was trained. Benchmark result {i:02} follows."),
                )
            })
            .collect();
        docs.push((
            "extra-a.md".into(),
            "This model was trained on a benchmark.".into(),
        ));
        docs.push((
            "extra-b.md".into(),
            "A benchmark for a trained model.".into(),
        ));
        let refs: Vec<(&str, &str)> = docs.iter().map(|(r, b)| (r.as_str(), b.as_str())).collect();
        let (out, _) = run(&refs);
        assert!(
            out.is_empty(),
            "'model'/'benchmark'/'trained' are in every document — no edge may \
             be drawn from them: {:?}",
            out.iter().map(|e| e.quote.clone()).collect::<Vec<_>>()
        );
    }

    /// A folder with a single readable document cannot produce a comparison.
    #[test]
    fn a_single_document_emits_nothing() {
        let (out, _) = run(&[("only.md", "Fermentation, lactobacillus, brine.")]);
        assert!(out.is_empty());
    }

    /// The 10% rule taken literally: in a folder of ten documents or fewer,
    /// two documents sharing a word share it with 20% of the corpus, and this
    /// detector asserts nothing. (This is also why the contract's five-note
    /// fixture keeps exactly the edge set it had before this detector existed.)
    #[test]
    fn a_corpus_too_small_for_distinctiveness_emits_nothing() {
        let (out, counters) = run(&[
            ("a.md", "Fermentation, lactobacillus, brine."),
            ("b.md", "Lactobacillus and brine again, fermentation."),
            ("c.md", "Unrelated: astronomy and parallax."),
            ("d.md", "Astronomy, parallax, telescopes."),
            ("e.md", "Nothing in common with anything."),
        ]);
        assert!(out.is_empty(), "{:?}", out.len());
        assert_eq!(
            counters.capped, 0,
            "nothing was refused by a budget — there were no candidates"
        );
    }

    /// Without stemming, a singular and its plural are two terms and one
    /// word. The floor counts word families so that pair cannot pass as two
    /// independent signals.
    #[test]
    fn singular_and_plural_are_one_word_not_two() {
        assert_eq!(families(&["trajectory", "trajectories"]), 1);
        assert_eq!(families(&["bid", "bids"]), 1);
        assert_eq!(families(&["recommender", "recommendations"]), 1);
        assert_eq!(families(&["player", "games"]), 2);
        assert_eq!(families(&["uav", "uavs", "trajectory"]), 2);
        let (out, _) = with_filler(
            &[
                ("one.md", "Trajectory planning. The trajectories matter."),
                ("two.md", "Trajectories and trajectory alike."),
            ],
            10,
        );
        assert!(
            out.iter()
                .all(|e| !e.quote.contains("one.md") && !e.quote.contains("two.md")),
            "one word in two spellings is not two shared terms: {:?}",
            out.iter().map(|e| e.quote.clone()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn stopword_list_is_sorted_for_binary_search() {
        let mut sorted = STOPWORDS.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, STOPWORDS.to_vec());
        assert!(is_stopword("the") && is_stopword("and") && !is_stopword("ferment"));
    }

    #[test]
    fn tokenizer_keeps_words_and_drops_numbers_and_noise() {
        let mut counts = BTreeMap::new();
        count_terms(
            "The starter, 2026, ferments; ferments again — Lactobacillus.",
            &mut counts,
        );
        assert_eq!(counts.get("ferments"), Some(&2));
        assert_eq!(counts.get("lactobacillus"), Some(&1), "lowercased");
        assert_eq!(counts.get("the"), None, "stopword");
        assert_eq!(counts.get("2026"), None, "pure number");
    }
}
