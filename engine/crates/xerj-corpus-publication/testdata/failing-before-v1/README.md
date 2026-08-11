# Failing-before semantic controls

This bundle records four deliberately injected controls against commit
`de1c63d90cbc4bc5a77faadf50220f1705f9ca37`, tree
`02390ea1fc5e9bb664614dbfb9f7a6507b0b3ce0`:

1. collapsing generation into desired sequence;
2. removing replay-tuple cardinality validation, exercised by one removed and
   one extra tuple;
3. removing replay-tuple operation-count validation; and
4. restoring the historical mutation ledger from before its seven causal-stage
   attestations.

These are **deliberate semantic controls**. The upstream base has no
`xerj-corpus-publication` package, so an unmodified-upstream before/after run is
not possible and is not claimed. “Before” means the exact final package with
only the checked-in patch applied. “After” means the exact unmodified subject.

## Reproduction

Start from the subject commit in a clean detached worktree. For each control:

1. verify the patch SHA-256 from `evidence.json`;
2. apply only that patch with `git apply`;
3. run each `mutated_before` command with a dedicated `CARGO_TARGET_DIR` and
   record its nonzero exit;
4. restore the patched file from the subject commit;
5. run each `unmodified_after` command and record exit zero; and
6. remove the dedicated target directory.

The historical-ledger control changes only
`testdata/review11-v1/mutations.json`; it does not regenerate or edit the
generator, provenance, or oracle attestation source.

## Normalization

`cargo-test-log/1` is an ordered, deterministic transform:

1. decode UTF-8 and normalize CRLF or CR to LF;
2. remove ANSI CSI sequences;
3. discard compiler/cache output before the first `running N test(s)` line;
4. replace the numeric panic thread ID with `<THREAD>`;
5. replace test-harness elapsed seconds with `<DURATION>`;
6. remove trailing horizontal whitespace and end with one LF.

Normalization never changes test names, assertion values, source-relative
locations, pass/fail counts, or cargo rerun hints. Passing controls use either
one exact test or `--test-threads=1`, so output order is stable. Raw logs,
absolute paths, target contents, timestamps, and machine-local references are
deliberately excluded from this bundle.
