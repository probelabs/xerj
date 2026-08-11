//! The security tooling this repo documents must be tooling that actually runs.
//!
//! Issue #207: `user-feedback/09-security/cves-and-vulnerabilities.md` advertised
//! `cargo-audit` "for dependency vulnerability scanning" and "fuzz testing
//! (cargo-fuzz) on all input parsing paths". Neither existed — no audit job, no
//! fuzz job, no fuzz target anywhere in the tree. A reader comparing XERJ against
//! the incumbent's CVE history priced in a fuzzed parser surface that was not
//! there, which is a worse failure than admitting the gap.
//!
//! Prose cannot be unit-tested, but *wiring* can, and the wiring is what the
//! prose is a claim about. These tests read the workflow file, the fuzz crate and
//! the doc off disk and fail if they disagree. All four fail at the commit that
//! filed #207.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Walk up from this crate to the repository root.
///
/// Deliberately panics rather than skipping: a check that quietly turns into a
/// no-op when it cannot find its inputs is exactly the tautological pass #207
/// complains about elsewhere in this repo's test suite.
fn repo_root() -> PathBuf {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        if dir.join(".github/workflows/ci.yml").is_file() {
            return dir;
        }
        assert!(
            dir.pop(),
            "no .github/workflows/ci.yml above {} — this test must run from a \
             repository checkout, and must not pass by failing to look",
            env!("CARGO_MANIFEST_DIR"),
        );
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// `[[bin]] name = "…"` entries in the fuzz crate — the definitive list of
/// harnesses that exist, taken from the manifest rather than from a list pasted
/// into this test.
fn declared_fuzz_targets(root: &Path) -> BTreeSet<String> {
    let manifest = root.join("engine/fuzz/Cargo.toml");
    assert!(
        manifest.is_file(),
        "engine/fuzz/Cargo.toml is missing: the repo documents cargo-fuzz harnesses, \
         so the crate that holds them has to exist"
    );
    let text = read(&manifest);
    let mut in_bin = false;
    let mut names = BTreeSet::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_bin = line == "[[bin]]";
            continue;
        }
        if in_bin {
            if let Some(rest) = line.strip_prefix("name") {
                if let Some(value) = rest.split('=').nth(1) {
                    names.insert(value.trim().trim_matches('"').to_string());
                }
            }
        }
    }
    names
}

#[test]
fn cargo_audit_runs_in_ci() {
    let root = repo_root();
    let ci = read(&root.join(".github/workflows/ci.yml"));

    assert!(
        ci.contains("security-audit:"),
        "no `security-audit` job in ci.yml, but the security docs say cargo-audit \
         scans our dependencies"
    );
    assert!(
        ci.contains("cargo audit"),
        "the `security-audit` job never invokes `cargo audit`"
    );
    assert!(
        ci.contains("cargo install cargo-audit"),
        "the audit job does not install cargo-audit, so `cargo audit` would be a \
         command-not-found — which some runners report as a skipped step rather \
         than a failure"
    );
}

#[test]
fn every_fuzz_target_has_a_harness_and_seeds() {
    let root = repo_root();
    let targets = declared_fuzz_targets(&root);

    assert!(
        !targets.is_empty(),
        "engine/fuzz declares no [[bin]] targets — the cargo-fuzz claim would be \
         satisfied by an empty crate"
    );

    for target in &targets {
        let harness = root.join(format!("engine/fuzz/fuzz_targets/{target}.rs"));
        assert!(
            harness.is_file(),
            "fuzz target `{target}` is declared in Cargo.toml but {} does not exist",
            harness.display()
        );

        let seed_dir = root.join(format!("engine/fuzz/seeds/{target}"));
        let seeds = std::fs::read_dir(&seed_dir)
            .map(|d| d.flatten().count())
            .unwrap_or(0);
        assert!(
            seeds > 0,
            "fuzz target `{target}` has no seeds at {}. Starting from the empty \
             input, a bounded CI run reaches almost none of the parser, so the \
             target would pass without proving anything",
            seed_dir.display()
        );
    }
}

#[test]
fn ci_executes_the_fuzz_harnesses() {
    let root = repo_root();
    let ci = read(&root.join(".github/workflows/ci.yml"));
    let script_rel = ".github/scripts/fuzz-smoke.sh";

    assert!(
        ci.contains("fuzz:"),
        "no `fuzz` job in ci.yml — harnesses that exist but never run are the same \
         false claim in a different place"
    );
    assert!(
        ci.contains(script_rel),
        "the fuzz job does not run {script_rel}"
    );

    let script = root.join(script_rel);
    assert!(script.is_file(), "{script_rel} is missing");
    let body = read(&script);

    // The script must discover targets from the crate. A hardcoded list is how
    // a new harness ends up shipped-but-never-executed.
    assert!(
        body.contains("fuzz list"),
        "{script_rel} must enumerate targets with `cargo fuzz list` so a newly \
         added harness cannot be silently left out of the run"
    );
}

#[test]
fn the_security_docs_claim_only_what_ci_does() {
    let root = repo_root();
    let ci = read(&root.join(".github/workflows/ci.yml"));

    for doc_rel in [
        "user-feedback/09-security/cves-and-vulnerabilities.md",
        "SECURITY.md",
    ] {
        let doc = read(&root.join(doc_rel));
        if doc.contains("cargo-audit") {
            assert!(
                ci.contains("cargo audit"),
                "{doc_rel} claims cargo-audit; CI does not run it"
            );
        }
        if doc.contains("cargo-fuzz") {
            assert!(
                !declared_fuzz_targets(&root).is_empty(),
                "{doc_rel} claims cargo-fuzz; the tree has no fuzz targets"
            );
        }
    }

    // The specific overclaim #207 was filed about, hunted across every prose
    // file rather than just this one — the point of the issue is that the claim
    // must not survive anywhere, and copy gets copied.
    //
    // Coverage is five named parsers, not "all" of them, and the difference is
    // the whole point: a reader is deciding whether to trust a parser surface.
    let mut offenders = Vec::new();
    scan_prose(&root, &mut |path, text| {
        // CHANGELOG.md quotes the retired claim to say it was retired. A
        // changelog is a record of what happened, not a statement about what
        // the software does now, and it is the one file where the sentence
        // appearing is correct.
        if path.file_name().is_some_and(|n| n == "CHANGELOG.md") {
            return;
        }
        if text.contains("all input parsing paths") {
            offenders.push(path.to_path_buf());
        }
    });
    assert!(
        offenders.is_empty(),
        "these files still claim fuzz coverage of *all* input parsing paths; list \
         the parsers actually covered instead: {offenders:#?}"
    );
}

/// Visit every prose file (`.md`, `.html`, `.txt`) that ships in this repo.
///
/// Skips VCS, build and dependency directories, and the fuzz inputs — those
/// hold adversarial data, not claims about the product.
fn scan_prose(dir: &Path, visit: &mut impl FnMut(&Path, &str)) {
    const SKIP: [&str; 9] = [
        ".git",
        "target",
        "node_modules",
        "seeds",
        "corpus",
        "artifacts",
        ".venv",
        "vendor",
        // Gitignored local clone of upstream Elasticsearch. Not our prose, and
        // large enough to make this test feel broken on a dev machine.
        "es-reference",
    ];
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if SKIP.contains(&name.as_ref()) {
            continue;
        }
        if path.is_dir() {
            scan_prose(&path, visit);
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("md" | "html" | "txt")
        ) {
            if let Ok(text) = std::fs::read_to_string(&path) {
                visit(&path, &text);
            }
        }
    }
}
