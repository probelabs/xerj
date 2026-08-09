//! Docs-vs-source drift guard for the published capability lists (issue #211).
//!
//! A source review concluded XERJ had no pipeline aggregations. It has fifteen.
//! The reviewer had read a hand-maintained list that stopped at `composite`,
//! and nearly opened a roadmap item to build a feature that already shipped.
//! The same lists advertised `has_child` / `has_parent`, which the parser
//! rejects with a 400.
//!
//! The root cause is that "what XERJ supports" was written down in prose in
//! several places and derived from the code in none of them. It is now derived
//! in exactly one place per subsystem —
//! [`xerj_query::parser::SUPPORTED_QUERY_TYPES`] and
//! [`xerj_engine::aggs::SUPPORTED_AGG_TYPES`], each pinned to its own dispatch
//! table by a unit test — and every published list is a marked region checked
//! against those constants here.
//!
//! Adding a query type or an aggregation now fails this test until the docs
//! are updated. That is the point: the failure is cheap, and the wrong
//! conclusion it prevents is not.

use std::collections::BTreeSet;
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

/// Every documentation file that carries a machine-checked capability list.
///
/// Paths are repo-relative. A file listed here that has lost its markers is a
/// hard failure, not a skip — a silently unchecked doc is the state this test
/// exists to end.
const CHECKED_DOCS: &[&str] = &["engine/README.md", "landing/llms-full.txt"];

/// Pull the names out of a `<!-- generated:<section> -->` … `<!-- /generated:<section> -->`
/// region.
///
/// Only backtick-delimited tokens count, so the region can carry readable
/// family labels ("Full-text:", "Pipeline:") without them being mistaken for
/// capability names. Anything backticked that is not an ES type name is a hard
/// error rather than a silently mis-parsed entry.
fn documented(doc: &str, path: &str, section: &str) -> BTreeSet<String> {
    let open = format!("<!-- generated:{section} -->");
    let close = format!("<!-- /generated:{section} -->");

    let start = doc.find(&open).unwrap_or_else(|| {
        panic!(
            "{path} has no `{open}` marker. Every capability list is generated \
             from the source constants and delimited by these markers; if the \
             section was renamed or removed, update CHECKED_DOCS/SECTIONS in \
             this test rather than leaving the list unchecked."
        )
    }) + open.len();
    let end = doc[start..]
        .find(&close)
        .unwrap_or_else(|| panic!("{path} opens `{open}` but never closes it with `{close}`"))
        + start;

    let region = &doc[start..end];
    let mut names = BTreeSet::new();
    let mut rest = region;
    while let Some(o) = rest.find('`') {
        let after = &rest[o + 1..];
        let Some(c) = after.find('`') else {
            panic!("{path} section `{section}` has an unclosed backtick");
        };
        let token = after[..c].trim();
        assert!(
            !token.is_empty()
                && token
                    .chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_'),
            "{path} section `{section}` contains `{token}`, which is not a bare type name. \
             The region is machine-checked and may hold only backticked type names plus \
             plain-text family labels — put prose outside the markers."
        );
        names.insert(token.to_string());
        rest = &after[c + 1..];
    }
    names
}

fn assert_section(section: &str, expected: &[&str]) {
    let root = repo_root();
    let expected: BTreeSet<String> = expected.iter().map(|s| s.to_string()).collect();

    for rel in CHECKED_DOCS {
        let path = root.join(rel);
        let doc = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        let listed = documented(&doc, rel, section);

        let missing: Vec<_> = expected.difference(&listed).collect();
        let extra: Vec<_> = listed.difference(&expected).collect();
        assert!(
            missing.is_empty() && extra.is_empty(),
            "{rel} section `{section}` has drifted from the source of truth.\n  \
             implemented but undocumented: {missing:?}\n  \
             documented but not implemented: {extra:?}\n  \
             The lists are generated: copy them from the constants in \
             xerj-query/src/parser.rs and xerj-engine/src/aggs.rs."
        );
    }
}

#[test]
fn documented_query_types_match_the_parser() {
    assert_section("query-types", xerj_query::parser::SUPPORTED_QUERY_TYPES);
}

/// The other direction of the same defect: types the docs must show as
/// *rejected*, so nobody plans around a `has_child` that answers 400.
#[test]
fn documented_rejected_query_types_match_the_parser() {
    assert_section(
        "rejected-query-types",
        xerj_query::parser::REJECTED_QUERY_TYPES,
    );
}

#[test]
fn documented_agg_types_match_the_engine() {
    assert_section("agg-types", xerj_engine::aggs::SUPPORTED_AGG_TYPES);
}

/// The markers are load-bearing, so prove the extractor actually rejects a
/// drifted list rather than passing on an empty or unparsed region.
#[test]
fn the_extractor_notices_drift() {
    let doc = "<!-- generated:query-types -->\n`match`, `term`\n<!-- /generated:query-types -->";
    let listed = documented(doc, "synthetic", "query-types");
    assert_eq!(
        listed,
        ["match", "term"]
            .iter()
            .map(|s| s.to_string())
            .collect::<BTreeSet<_>>()
    );
    assert!(!listed.contains("bucket_script"));
}

/// The crate map is the same defect in a different list: it named eleven of the
/// sixteen crates, so an agent orienting from it never learned that
/// `xerj-autoindex` — the flagship feature — exists.
///
/// Checked by containment rather than by markers, because the map is a table
/// with a prose column: every `crates/*` workspace member must be named in it,
/// and every crate it names must still be a member.
#[test]
fn the_readme_crate_map_lists_every_workspace_crate() {
    let root = repo_root();
    let manifest =
        std::fs::read_to_string(root.join("engine/Cargo.toml")).expect("engine manifest");
    let readme_path = root.join("engine/README.md");
    let readme = std::fs::read_to_string(&readme_path).expect("engine README");

    let members: BTreeSet<String> = manifest
        .lines()
        .filter_map(|l| l.trim().strip_prefix("\"crates/"))
        .filter_map(|l| l.split('"').next())
        .map(str::to_string)
        .collect();
    assert!(
        members.len() > 10,
        "only {} workspace members parsed out of engine/Cargo.toml — the manifest \
         layout changed and this check is reading nothing",
        members.len()
    );

    let map_start = readme
        .find("### Crate Map")
        .expect("engine/README.md lost its `### Crate Map` heading");
    let map = &readme[map_start..];
    let map = &map[..map.find("\n### ").unwrap_or(map.len())];

    // Only the first cell of a table row counts. A crate merely *mentioned* in
    // the surrounding prose is not documented, and an earlier draft of this
    // test accepted exactly that — the guard has to be harder to satisfy than
    // the thing it guards.
    let rows: BTreeSet<String> = map
        .lines()
        .filter_map(|l| l.trim().strip_prefix("| `"))
        .filter_map(|l| l.split('`').next())
        .map(str::to_string)
        .collect();

    let undocumented: Vec<_> = members.difference(&rows).collect();
    assert!(
        undocumented.is_empty(),
        "engine/README.md's crate map has no row for {undocumented:?} — every crate \
         under engine/crates/ needs its own `| `<crate>` | purpose |` row"
    );

    let phantom: Vec<_> = rows
        .iter()
        .filter(|r| r.starts_with("xerj-") && !members.contains(*r))
        .collect();
    assert!(
        phantom.is_empty(),
        "engine/README.md's crate map has a row for {phantom:?}, which is not a workspace member"
    );
}

/// A backticked qualifier — the shape the old prose lists used, e.g.
/// `knn (HNSW-served unfiltered / exact filtered)` — must fail loudly rather
/// than be absorbed as a capability name that then never matches anything.
/// Accepted-and-ignored input is the failure mode this repo tracks in issue
/// #204; the docs guard must not add a documentation-flavoured instance of it.
#[test]
#[should_panic(expected = "is not a bare type name")]
fn a_backticked_qualifier_inside_a_region_is_refused() {
    let doc =
        "<!-- generated:query-types -->\n`knn (HNSW-served)`\n<!-- /generated:query-types -->";
    documented(doc, "synthetic", "query-types");
}
