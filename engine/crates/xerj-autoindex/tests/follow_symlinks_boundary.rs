//! Issue #438: `--follow-symlinks` used to defeat the hidden-file rule and
//! walk out of the indexed root entirely.
//!
//! The hidden-name rule in `walk.rs` judges the name the walk arrived by. With
//! links followed that is not the file: a link called `notes.txt` pointing at
//! `.secretdir/k.txt` passed the rule and its contents were indexed, and a link
//! called `shared` pointing anywhere at all dragged that tree in under the
//! root's rel paths. The run then printed `hidden:dotfile dirs: 1`, which was
//! true of the traversal and false of the outcome — the operator was told the
//! hidden directory had been pruned while its contents were being indexed
//! through the link.
//!
//! Both halves are tested here through the public `walk_reporting`, against a
//! real filesystem with real symlinks, because the defect is entirely about
//! what the OS resolves and nothing about it is visible to a unit test over
//! synthetic paths.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;
use xerj_autoindex::ignore_rules::{IgnoreOptions, ESCAPED_ROOT_RULE, HIDDEN_RULE};
use xerj_autoindex::walk::walk_reporting;

/// root/
///   README.md              visible, ordinary
///   .secretdir/k.txt       the secret
///   notes.txt          ->  .secretdir/k.txt        (visible name, hidden target)
///   chain.txt          ->  hop.txt -> .secretdir/k.txt
///   plain-link.txt     ->  README.md               (must still be indexed)
///   shared             ->  ../outside              (escapes the root)
///   sub/deep.md            visible, ordinary, below a directory
/// outside/etc/shadow       must never appear
fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    // `tempfile` names its directories `.tmpXXXXXX`, a hidden component. Every
    // fixture is anchored one level below it so the harness's own name does not
    // trip the dotfile rule the tests are about.
    let base = dir.path().join("base");
    std::fs::create_dir_all(&base).expect("base");
    let root = base.join("root");
    let outside = base.join("outside");

    fs::create_dir_all(root.join(".secretdir")).unwrap();
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::create_dir_all(outside.join("etc")).unwrap();

    fs::write(root.join("README.md"), "# readme\n").unwrap();
    fs::write(root.join("sub/deep.md"), "deep\n").unwrap();
    fs::write(root.join(".secretdir/k.txt"), "INNER SECRET\n").unwrap();
    fs::write(outside.join("etc/shadow"), "root:$6$OUTSIDE$hash:...\n").unwrap();

    symlink(".secretdir/k.txt", root.join("notes.txt")).unwrap();
    symlink(".secretdir/k.txt", root.join("hop.txt")).unwrap();
    symlink("hop.txt", root.join("chain.txt")).unwrap();
    symlink("README.md", root.join("plain-link.txt")).unwrap();
    symlink(&outside, root.join("shared")).unwrap();

    (dir, root)
}

fn walk_rels(
    root: &Path,
    follow: bool,
) -> (Vec<String>, xerj_autoindex::ignore_rules::IgnoreReport) {
    let (files, report) =
        walk_reporting(root, follow, IgnoreOptions::default()).expect("walk succeeds");
    let mut rels: Vec<String> = files.into_iter().map(|f| f.rel).collect();
    rels.sort();
    (rels, report)
}

/// The whole point: with links followed, no secret and nothing from outside.
#[test]
fn following_links_indexes_neither_the_secret_nor_anything_outside_the_root() {
    let (_dir, root) = fixture();

    let (rels, report) = walk_rels(&root, true);

    // A file that is only reachable by resolving a link into a hidden
    // directory must not be indexed under any name.
    for leaked in ["notes.txt", "hop.txt", "chain.txt"] {
        assert!(
            !rels.iter().any(|r| r == leaked),
            "`{leaked}` resolves into .secretdir and was indexed anyway: {rels:?}"
        );
    }

    // Nothing from outside the root, under any rel path.
    assert!(
        !rels.iter().any(|r| r.starts_with("shared")),
        "content from outside the indexed root was walked in: {rels:?}"
    );

    // And the ordinary files are all still there — a fix that indexes nothing
    // also passes the two assertions above.
    assert_eq!(
        rels,
        vec![
            "README.md".to_string(),
            "plain-link.txt".to_string(),
            "sub/deep.md".to_string()
        ],
        "the visible tree must be unchanged, including a link to a VISIBLE file"
    );

    // The report has to say what happened, because "my file vanished" is
    // answered from it. Silently correct is still a lie about the run.
    let hidden = report.by_rule.get(HIDDEN_RULE).copied().unwrap_or_default();
    assert!(
        hidden.files >= 3,
        "the three links into .secretdir must be reported as hidden, got {hidden:?} in {report:?}"
    );
    let escaped = report
        .by_rule
        .get(ESCAPED_ROOT_RULE)
        .copied()
        .unwrap_or_default();
    assert_eq!(
        escaped.dirs, 1,
        "the link out of the root must be reported under `{ESCAPED_ROOT_RULE}`: {report:?}"
    );
}

/// The default path must be untouched by the fix — links are not followed, so
/// the link files themselves are the entries and nothing resolves.
#[test]
fn not_following_links_is_unchanged() {
    let (_dir, root) = fixture();

    let (rels, report) = walk_rels(&root, false);

    assert_eq!(
        rels,
        vec!["README.md".to_string(), "sub/deep.md".to_string()],
        "with links unfollowed only real files are entries: {report:?}"
    );
    assert!(
        !report.by_rule.contains_key(ESCAPED_ROOT_RULE),
        "nothing escapes when nothing is followed: {report:?}"
    );
}

/// A link pointing at a visible file INSIDE the root is the case the fix must
/// not break, and the one a blunt "refuse all symlinks" would.
#[test]
fn a_link_to_a_visible_file_inside_the_root_is_still_indexed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("base").join("root");
    fs::create_dir_all(root.join("docs")).unwrap();
    fs::write(root.join("docs/real.md"), "real\n").unwrap();
    symlink("docs/real.md", root.join("alias.md")).unwrap();

    let (rels, _) = walk_rels(&root, true);
    assert_eq!(
        rels,
        vec!["alias.md".to_string(), "docs/real.md".to_string()],
        "following a link to a visible in-root file must keep indexing it"
    );
}

/// A link to a directory inside the root: still walked, and the hidden rule
/// still applies to what is under it.
#[test]
fn a_link_to_a_directory_inside_the_root_is_walked_and_still_hides_dotfiles() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("base").join("root");
    fs::create_dir_all(root.join("real/.hidden")).unwrap();
    fs::write(root.join("real/open.md"), "open\n").unwrap();
    fs::write(root.join("real/.hidden/secret.md"), "secret\n").unwrap();
    symlink("real", root.join("mirror")).unwrap();

    let (rels, report) = walk_rels(&root, true);
    assert!(
        rels.contains(&"mirror/open.md".to_string()),
        "an in-root directory link must still be walked: {rels:?}"
    );
    assert!(
        !rels.iter().any(|r| r.contains(".hidden")),
        "the hidden rule must survive the link: {rels:?} {report:?}"
    );
}

/// A broken link has no target, so it cannot leak one. It must not abort the
/// walk or be reported as an escape.
#[test]
fn a_broken_link_is_not_an_escape() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("base").join("root");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("real.md"), "real\n").unwrap();
    symlink("nowhere-at-all", root.join("dangling.md")).unwrap();

    let (rels, report) = walk_rels(&root, true);
    assert_eq!(rels, vec!["real.md".to_string()], "{report:?}");
    assert!(
        !report.by_rule.contains_key(ESCAPED_ROOT_RULE),
        "a dangling link is not an escape: {report:?}"
    );
}

/// The escape hatch. Refusing every out-of-root target is right by default and
/// wrong as the only option: for a vendored sibling checkout or a mounted
/// volume, following the link outward IS the reason the flag was turned on.
/// `--follow-symlinks-outside-root` waives the root boundary — and nothing
/// else. The hidden-name rule still applies to whatever the link resolves to,
/// which is the half that keeps `.ssh` out.
#[test]
fn the_outside_root_opt_in_waives_the_boundary_and_nothing_else() {
    let (_dir, root) = fixture();

    let (refused, _) = xerj_autoindex::walk::walk_reporting_opts(
        &root,
        true,
        false,
        xerj_autoindex::ignore_rules::IgnoreOptions::default(),
    )
    .expect("walk succeeds");
    assert!(
        !refused.iter().any(|f| f.rel.starts_with("shared")),
        "default must still refuse the out-of-root link"
    );

    let (allowed, report) = xerj_autoindex::walk::walk_reporting_opts(
        &root,
        true,
        true,
        xerj_autoindex::ignore_rules::IgnoreOptions::default(),
    )
    .expect("walk succeeds");
    let rels: Vec<&str> = allowed.iter().map(|f| f.rel.as_str()).collect();
    assert!(
        rels.iter().any(|r| r.starts_with("shared")),
        "the opt-in must actually follow the link outward: {rels:?}"
    );

    // The half that must NOT be waived, in-root.
    for leaked in ["notes.txt", "hop.txt", "chain.txt"] {
        assert!(
            !rels.contains(&leaked),
            "`{leaked}` resolves into .secretdir — the hidden rule is not part \
             of the opt-in: {rels:?} {report:?}"
        );
    }
}

/// The same half, OUTSIDE the root — which is the only place the opt-in
/// actually changes anything, and where the first version of it failed.
///
/// `escaped_or_hidden_target` answered `OutsideRoot` before it looked for a
/// hidden component, so the opt-in admitted the entry unexamined and
/// `keys.txt -> ../outside/.ssh/id_rsa` was indexed with the key as its body.
/// That is #438 reopened through the escape hatch built to be safe, and the
/// test above did not catch it because every one of its links points inward.
#[test]
fn the_opt_in_does_not_admit_a_dotted_path_outside_the_root() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("base");
    fs::create_dir_all(&base).unwrap();
    let root = base.join("root");
    let outside = base.join("outside");
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::create_dir_all(outside.join(".ssh")).unwrap();
    fs::create_dir_all(outside.join("pkg")).unwrap();
    fs::write(root.join("README.md"), "readme\n").unwrap();
    fs::write(outside.join(".ssh/id_rsa"), "PRIVATE KEY\n").unwrap();
    fs::write(outside.join("pkg/lib.rs"), "pub fn f() {}\n").unwrap();

    symlink(outside.join(".ssh/id_rsa"), root.join("keys.txt")).unwrap();
    symlink(outside.join(".ssh"), root.join("conf")).unwrap();
    symlink(outside.join("pkg"), root.join("shared-lib")).unwrap();

    let (files, report) = xerj_autoindex::walk::walk_reporting_opts(
        &root,
        true,
        true,
        xerj_autoindex::ignore_rules::IgnoreOptions::default(),
    )
    .expect("walk succeeds");
    let rels: Vec<&str> = files.iter().map(|f| f.rel.as_str()).collect();

    assert!(
        !rels
            .iter()
            .any(|r| r.starts_with("keys.txt") || r.starts_with("conf")),
        "the opt-in must not admit a target under a dotted directory: {rels:?} {report:?}"
    );
    assert!(
        rels.iter().any(|r| r.starts_with("shared-lib")),
        "an ordinary out-of-root target is what the opt-in exists for: {rels:?}"
    );
}

/// Two properties the escape hatch got wrong, both found by verification.
///
/// 1. An entry the waiver ADMITS must carry the resolved path. The arm was
///    empty, so `FileEntry::path` stayed the walk path and every later `open()`
///    re-followed the link — the decision made about one file, the bytes read
///    from another, for everything the flag ingests.
/// 2. An out-of-root target refused for a dotted component must be reported as
///    what it is. It was recorded under `hidden:dotfile`, so an operator was
///    told to look for a dotfile inside their own folder and
///    `symlink:outside-root` vanished from the report — on the DEFAULT path,
///    not just under the flag.
#[test]
fn out_of_root_entries_carry_the_resolved_path_and_are_reported_as_escapes() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("base");
    fs::create_dir_all(&base).unwrap();
    let root = base.join("root");
    let outside = base.join("outside");
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::create_dir_all(outside.join("pkg")).unwrap();
    fs::create_dir_all(outside.join(".ssh")).unwrap();
    fs::write(root.join("README.md"), "readme\n").unwrap();
    fs::write(outside.join("pkg/lib.rs"), "pub fn f() {}\n").unwrap();
    fs::write(outside.join(".ssh/id_rsa"), "PRIVATE KEY\n").unwrap();

    symlink(outside.join("pkg"), root.join("shared-lib")).unwrap();
    symlink(outside.join(".ssh/id_rsa"), root.join("keys.txt")).unwrap();

    // (1) admitted out-of-root entries store the resolution.
    let (files, _) = xerj_autoindex::walk::walk_reporting_opts(
        &root,
        true,
        true,
        xerj_autoindex::ignore_rules::IgnoreOptions::default(),
    )
    .expect("walk succeeds");
    let lib = files
        .iter()
        .find(|f| f.rel == "shared-lib/lib.rs")
        .expect("the opt-in must follow an ordinary out-of-root link");
    assert_eq!(
        lib.path,
        outside.join("pkg/lib.rs").canonicalize().unwrap(),
        "the stored path must be the resolved one, or the read re-follows the link"
    );

    // (2) the dotted out-of-root refusal is an escape, not a dotfile — and it
    // is the DEFAULT path that has to say so.
    let (_, report) = xerj_autoindex::walk::walk_reporting_opts(
        &root,
        true,
        false,
        xerj_autoindex::ignore_rules::IgnoreOptions::default(),
    )
    .expect("walk succeeds");
    assert!(
        !report.by_rule.contains_key(HIDDEN_RULE),
        "there is no dotfile in the operator's folder to blame: {report:?}"
    );
    let escaped = report
        .by_rule
        .get(ESCAPED_ROOT_RULE)
        .copied()
        .unwrap_or_default();
    // `keys.txt` is a file link and `shared-lib` a directory link, so they land
    // in different counters — one each, not two files.
    assert_eq!(
        (escaped.files, escaped.dirs),
        (1, 1),
        "both links left the root: {report:?}"
    );
}
