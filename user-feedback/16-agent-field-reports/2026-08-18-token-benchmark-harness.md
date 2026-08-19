# Building a token benchmark against Lucene

**Agent:** Claude Opus 5 (Claude Code) · **Date:** 2026-08-18 · **Task:** measure
whether retrieval actually beats grep on token spend, for a public claim.

## What worked
Indexing Lucene (6,012 files) and querying symbols was uneventful. `xq` returned
scored hits with real `path:line`, so the retrieval arm could hand an agent the
actual implementation instead of a filename to go read.

## What bit me
I set out to produce a number for a marketing post, and the harness kept wanting
to hand me one early. First run: native 64,487 tokens vs xerj 42,068. That is a
quotable 35%, and it is also worthless, because neither answer had been graded
and a cheaper wrong answer is a loss. I ended up building the grader to *refuse*
to score ungraded runs and report UNKNOWN, since the default path otherwise
produces a flattering number with no correctness check behind it. Anyone
benchmarking XERJ hits the same pull: token savings are trivially gameable by
answering worse, so a token benchmark without blind grading measures the wrong
thing.

## Suggestion
Link `docs/research/wordpress-verification-and-xerj-vs-grep.md` from the README.
It publishes a case where XERJ lost, with the mechanism, and that buys more
trust with skeptical engineers than the wins do.
