//! Release-notes drift guard: a shipped doc may not advertise unshipped things
//! (issue #474).
//!
//! A file that ships inside the release tag may not describe a capability as
//! arriving in a future release. Such a sentence cannot be checked at review
//! time and is wrong by construction afterwards: either the feature has not
//! landed, and a reader of the tag is told to run something that does not
//! exist, or it has landed, and the text is stale.
//!
//! Both of these were published — the second in a tagged release, the first
//! on `main` — which is why this exists:
//!
//! - `README.md` and `landing/llms.txt` documented `xerj feedback` as "on
//!   `main` now; ships in the next release" while it sat in an unmerged PR, so
//!   the release's own community CTA opened with a command that exits
//!   `unknown argument` — aimed at exactly the agents the page recruits.
//!
//! - `landing/llms.txt` and `landing/llms-full.txt` said `--port` "is on
//!   `main` and ships in the next release", and `llms.txt` that `xerj --help`
//!   lists it "on a `main` build only". It had shipped in v1.0.0-rc.17. An
//!   agent hitting a port collision — the scenario that paragraph exists for —
//!   was told to hand-write a three-key TOML file while holding a binary that
//!   takes `--port 9300`.
//!
//! Write instead what is true of *this* binary, anchored to a version rather
//! than to the moment of reading — "since v1.0.0-rc.17", or "v1.0.0-rc.17 does
//! not have it". "yet", "now", "today", "currently" and "in review" all
//! expire; a version number does not.
//!
//! `CHANGELOG.md` is exempt: an `[Unreleased]` section is exactly where
//! forward-looking statements belong.
//!
//! This narrows the class rather than closing it — it is a list of known
//! phrasings, not a rule about meaning. Issue #474 tracks the API-backed
//! checks that would do more.

use std::path::{Path, PathBuf};

/// Repository root, derived from this crate's manifest directory
/// (`<repo>/engine/crates/xerj-engine`).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("crate manifest dir must be <repo>/engine/crates/xerj-engine")
        .to_path_buf()
}

/// The user-facing files that ship inside the release tag and are read by
/// someone holding a released binary.
///
/// `CHANGELOG.md` is deliberately absent — see the module docs.
const SHIPPED_DOCS: &[&str] = &[
    "README.md",
    "landing/llms.txt",
    "landing/llms-full.txt",
    "ROADMAP.md",
];

/// Phrases that promise a future release.
///
/// Matched case-insensitively against the whole line. Kept literal and short:
/// this gate is worth having only if a contributor can read a failure and know
/// immediately what to write instead, and only if it does not fire on prose
/// that merely mentions a version.
const FUTURE_RELEASE_PHRASES: &[&str] = &[
    "ships in the next release",
    "ships next release",
    "in the next release",
    "on `main` now",
    "on main now",
    "is on `main` and ships",
    "is on main and ships",
    "not yet in a tagged release",
    "on a `main` build only",
    "on a main build only",
];

/// Read a file under the repo root, or fail the test naming it.
fn read(root: &Path, rel: &str) -> String {
    let path = root.join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read {} for the docs drift gate: {e}",
            path.display()
        )
    })
}

/// A shipped doc must not tell the reader that something arrives later.
///
/// This is the assertion that would have caught both `xerj feedback` and
/// `--port`, on the commit that introduced each.
#[test]
fn no_shipped_doc_promises_a_future_release() {
    let root = repo_root();
    let mut offences: Vec<String> = Vec::new();

    for rel in SHIPPED_DOCS {
        let body = read(&root, rel);
        for (i, line) in body.lines().enumerate() {
            let lowered = line.to_lowercase();
            for phrase in FUTURE_RELEASE_PHRASES {
                if lowered.contains(phrase) {
                    offences.push(format!(
                        "{rel}:{} promises a future release ({phrase:?}):\n    {}",
                        i + 1,
                        line.trim()
                    ));
                }
            }
        }
    }

    assert!(
        offences.is_empty(),
        "A file that ships inside the release tag describes a capability as arriving \
         in a future release. That sentence is wrong by construction the moment the tag \
         is cut: either the feature has not landed and a reader of this tag is told to \
         run something that does not exist, or it has landed and this text is stale. \
         Both shipped in the rc.18 cycle (issue #474).\n\n\
         Write what is true of THIS binary instead, anchored to a version rather than \
         to the moment of reading — \"since v1.0.0-rc.17\", or \"v1.0.0-rc.17 does not \
         have it\". \"yet\", \"now\", \"today\" and \"in review\" all expire; a version \
         number does not. \
         Forward-looking notes belong in CHANGELOG's [Unreleased] section, which this \
         gate does not check.\n\n{}",
        offences.join("\n")
    );
}

/// The phrase list has to actually match the text that shipped, or this gate
/// is decoration.
///
/// Each sentence below really was published, and the list must keep catching
/// every one of them. Gutting the list fails here.
///
/// What this does NOT do, measured rather than assumed: it does not make every
/// individual entry unremovable. The entries overlap — `"in the next release"`
/// subsumes `"ships in the next release"` — and these pins are sentences,
/// while the sweep matches per line, so a sentence can stay caught by one
/// entry after another that covered a different *line* of it is deleted.
/// Pruning each entry in turn, **8 of the 10 pass silently**; only
/// `"not yet in a tagged release"` and ``"on a `main` build only"`` are the
/// sole cover for something pinned. So this test stops the list being emptied,
/// not the list being thinned, and someone who deletes a redundant entry can
/// reopen the exact wording of a past defect with CI green.
///
/// Making it stronger means pinning the real pre-fix *lines* rather than
/// reconstructed sentences, so entry necessity can be checked at the
/// granularity the sweep actually uses. That is worth doing and is not done
/// here; #474 carries it with the other gaps.
#[test]
fn the_phrase_list_catches_the_sentences_that_caused_this_gate() {
    // `landing/llms.txt`, before the fix — the `--port` claim, false for a
    // full release by the time it was found.
    let port_claim = "A `--port <PORT>` flag (which also claims `PORT+1` for native REST \
                      and `PORT+2` for gRPC) is on `main` and ships in the next release \
                      — `xerj --help` tells you which binary you have";
    // `README.md`, before the fix — the community CTA's headline command.
    let feedback_claim = "# The one-command path (on `main` now; ships in the next release):";
    // `landing/llms.txt`, before the fix — the `--help` availability claim.
    let help_claim = "That block works on every released binary. On a `main` build only, \
                      `xerj --help` also lists a `--port` flag";
    // `README.md`, before the fix — the same CTA in the contributing section.
    let contributing_claim = "The baseline contribution is one short field report: \
                              `xerj feedback --open-pr` (on `main` now, ships next release) \
                              or, today, a plain `gh pr create` adding one markdown file";
    // `landing/llms-full.txt`, before the fix — the same `--port` availability
    // claim as `port_claim`, published with `main` unbackticked. This is why the
    // list carries backtick-free twins: the sentence really did go out both ways.
    let full_port_claim =
        "each other), is on main and ships in the next release; `xerj --help` tells you";
    // `landing/llms.txt`, before the fix — the defence-by-analogy that pointed
    // at the `--port` paragraph as the model, and so seeded the second defect.
    let analogy_claim = "documented here the way the `--port` flag is: available on `main`, \
                         not yet in a tagged release; `xerj --help` is the authority for the \
                         binary in front of you";

    for (label, text) in [
        ("--port availability", port_claim),
        ("xerj feedback CTA", feedback_claim),
        ("--help availability", help_claim),
        ("contributing-section CTA", contributing_claim),
        ("defence-by-analogy", analogy_claim),
        ("--port availability, no backticks", full_port_claim),
    ] {
        let lowered = text.to_lowercase();
        assert!(
            FUTURE_RELEASE_PHRASES.iter().any(|p| lowered.contains(p)),
            "FUTURE_RELEASE_PHRASES no longer catches the {label} sentence that caused \
             issue #474. Pruning this list re-opens the defect it exists to close:\n    {text}"
        );
    }

    // "Some phrase still matches" is too weak on its own: the pinned sentences
    // overlap, so a subset of the list can still satisfy every pin while the
    // rest become free to delete — and with them, free to reintroduce. Every
    // entry must therefore at least still match something: it has to catch at
    // least one pinned sentence. (That is a floor, not a lock — this assert
    // cannot fire on a deletion, only on an entry that matches nothing.) An entry that catches nothing is either dead weight or a
    // phrasing whose example sentence went missing, and both are worth a
    // failure rather than silent rot.
    let corpus: String = [
        port_claim,
        feedback_claim,
        help_claim,
        contributing_claim,
        analogy_claim,
        full_port_claim,
    ]
    .join("\n")
    .to_lowercase();
    // The list carries a backtick-free twin of each phrase that shipped with
    // backticks, because the same sentence really did get published both ways —
    // `full_port_claim` above is the unbackticked form, pinned like any other.
    // The remaining twins have no separately published sentence of their own,
    // so they are justified against the corpus with backticks stripped rather
    // than by inventing prose nobody wrote.
    let corpus_plain = corpus.replace('`', "");
    let idle: Vec<&str> = FUTURE_RELEASE_PHRASES
        .iter()
        .copied()
        .filter(|p| !corpus.contains(p) && !corpus_plain.contains(p))
        .collect();
    assert!(
        idle.is_empty(),
        "these entries in FUTURE_RELEASE_PHRASES match none of the sentences pinned above, \
         so nothing proves they still work: {idle:?}\n\n\
         Either add the real sentence each one came from to the pins, or drop the entry. \
         A phrase with no example behind it is how this list rots back into a shape that \
         lets the rc.18 wording through again (issue #474)."
    );
}

/// The file list has to stay covered, or the gate is one deleted line from
/// inert.
///
/// Without this test, deleting the single line `"README.md",` makes the whole
/// pre-fix README pass the sweep — every offence in it, including the
/// `xerj feedback` block that opened the release's own community CTA, stops
/// being seen.
#[test]
fn the_file_list_still_covers_every_doc_that_ships_in_the_tag() {
    for required in [
        "README.md",
        "landing/llms.txt",
        "landing/llms-full.txt",
        "ROADMAP.md",
    ] {
        assert!(
            SHIPPED_DOCS.contains(&required),
            "{required} was dropped from SHIPPED_DOCS. Every file here ships inside the \
             release tag and is read by someone holding a released binary, so removing one \
             silently disables this gate for it — which is how a reader ends up back at a \
             command that does not exist (issue #474). Widening the list is welcome; \
             narrowing it is the defect."
        );
    }
}

// A phrase broad enough to hit ordinary prose is caught by the sweep itself,
// not by a separate canary.
//
// There was once an `ordinary_version_prose_is_not_an_offence` test here,
// holding hand-written example sentences the phrase list had to leave alone.
// It was redundant: adding `"release"` to `FUTURE_RELEASE_PHRASES` fails
// `no_shipped_doc_promises_a_future_release`, because that test reads the four
// real documents; the canary, holding a handful of hand-picked sentences,
// passed the same change. The sweep is the stronger false-positive detector
// and it comes for free — it measures the actual corpus rather than a sample
// somebody has to keep honest.
//
// Kept as a comment because the lesson is about what not to add back.
