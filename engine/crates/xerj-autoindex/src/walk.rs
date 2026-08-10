//! Inventory: recursive walk of the target folder.
//! Symlinks are NOT followed by default; with --follow-symlinks walkdir's
//! ancestor-loop detection keeps the traversal loop-safe.
//!
//! Ignore rules (`.gitignore`, `.xerjignore`, built-in build-output defaults)
//! are applied *during* the walk, not after it: an ignored directory is never
//! descended, so its contents cost nothing — no stat, no hash, no bulk body.
//! The matching itself lives in [`crate::ignore_rules`].

use crate::ignore_rules::{IgnoreOptions, IgnoreReport, IgnoreStack};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: PathBuf,
    /// Root-relative path with forward slashes — the stable `ax_path` value.
    pub rel: String,
    /// Reversible platform-native identity; unlike `rel`, never lossy.
    pub rel_id: String,
    /// True when the discovered path itself is a symlink (target metadata follows).
    pub is_symlink: bool,
    pub size: u64,
}

/// Walk with the default ignore rules on and the report discarded.
pub fn walk(root: &Path, follow_symlinks: bool) -> Result<Vec<FileEntry>> {
    walk_reporting(root, follow_symlinks, IgnoreOptions::default()).map(|(files, _)| files)
}

/// Walk, and say what was left out and why.
///
/// The returned [`IgnoreReport`] is the answer to "my files vanished" — it
/// names every rule that discarded something and how much. It is reported on
/// every run and, under `--dry-run`, also counts what was inside each pruned
/// directory.
pub fn walk_reporting(
    root: &Path,
    follow_symlinks: bool,
    ignore: IgnoreOptions,
) -> Result<(Vec<FileEntry>, IgnoreReport)> {
    let root_canon = root
        .canonicalize()
        .with_context(|| format!("resolve root folder {}", root.display()))?;
    if !root_canon.is_dir() {
        anyhow::bail!("{} is not a directory", root_canon.display());
    }
    let mut out = Vec::new();
    let mut stack = IgnoreStack::new(&root_canon, ignore);
    // Manual iteration rather than `filter_entry`, because the ignore stack has
    // to load a directory's ignore files *after* that directory is admitted and
    // *before* its children are judged. `skip_current_dir` on a just-yielded
    // directory prunes its contents — walkdir's own documented pattern, and the
    // same effect `filter_entry` had.
    let mut it = walkdir::WalkDir::new(&root_canon)
        .follow_links(follow_symlinks)
        .into_iter();
    while let Some(next) = it.next() {
        let entry = match next {
            Ok(e) => e,
            Err(e) => {
                eprintln!("walk: skipping unreadable entry: {e}");
                continue;
            }
        };
        let is_dir = entry.file_type().is_dir();
        // Retire the layers of directories this entry is no longer inside; see
        // `IgnoreStack::truncate_to` for why every entry needs it, not just
        // directories.
        stack.truncate_to(entry.depth());
        // SECURITY / hygiene: never index hidden files or descend into hidden
        // directories. Without this the walker happily indexed `.env` (secrets,
        // API tokens), `.git`, `.ssh`, `.aws`, and other dotfiles into a
        // queryable brain with no per-brain authorization — a real exposure for
        // the "point it at my project folder" use case. A hidden directory is
        // pruned before descending, so `.git/` is skipped whole. The root
        // itself (depth 0) is exempt so a brain over a dot-named folder still
        // works. `--no-ignore` does NOT turn this off: it is not one of the
        // ignore rules, it is the reason secrets stay out.
        if entry.depth() > 0
            && entry
                .file_name()
                .to_str()
                .is_some_and(|n| n.starts_with('.'))
        {
            stack.record_hidden(is_dir);
            if is_dir {
                it.skip_current_dir();
            }
            continue;
        }
        if is_dir {
            // The root is never judged by the rules below it: pointing
            // autoindex at a folder is an explicit instruction, and the note
            // explaining that it *would* have been ignored is on the report.
            if entry.depth() > 0 && stack.skip_dir(entry.path()).is_some() {
                it.skip_current_dir();
                continue;
            }
            stack.enter_dir(entry.path(), entry.depth());
            continue;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        if stack.skip_file(entry.path()).is_some() {
            continue;
        }
        let md = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let p = entry.path().to_path_buf();
        let rel = p
            .strip_prefix(&root_canon)
            .unwrap_or(&p)
            .to_string_lossy()
            .replace('\\', "/");
        let rel_id = stable_path_id(p.strip_prefix(&root_canon).unwrap_or(&p));
        out.push(FileEntry {
            path: p,
            rel,
            rel_id,
            is_symlink: entry
                .path()
                .symlink_metadata()
                .is_ok_and(|m| m.file_type().is_symlink()),
            size: md.len(),
        });
    }
    // Deterministic order for clustering / naming.
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok((out, stack.report))
}

#[cfg(unix)]
fn stable_path_id(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut out = String::from("unix:");
    for byte in path.as_os_str().as_bytes() {
        use std::fmt::Write;
        write!(out, "{byte:02x}").expect("write string");
    }
    out
}

#[cfg(windows)]
fn stable_path_id(path: &Path) -> String {
    use std::os::windows::ffi::OsStrExt;
    let mut out = String::from("windows:");
    for unit in path.as_os_str().encode_wide() {
        use std::fmt::Write;
        write!(out, "{unit:04x}").expect("write string");
    }
    out
}

#[cfg(not(any(unix, windows)))]
fn stable_path_id(path: &Path) -> String {
    format!("other:{}", path.to_string_lossy())
}

#[cfg(test)]
mod hidden_skip_tests {
    use super::walk;
    use std::fs;

    #[test]
    fn hidden_files_and_dirs_are_never_indexed() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        fs::write(root.join("README.md"), "hello").unwrap();
        fs::write(root.join(".env"), "SECRET=pat-abc123").unwrap();
        fs::create_dir(root.join(".git")).unwrap();
        fs::write(root.join(".git").join("config"), "[core]").unwrap();
        fs::create_dir(root.join("src")).unwrap();
        fs::write(root.join("src").join("main.rs"), "fn main() {}").unwrap();
        fs::write(root.join("src").join(".secret"), "nope").unwrap();

        let rels: Vec<String> = walk(root, false)
            .unwrap()
            .into_iter()
            .map(|e| e.rel)
            .collect();
        assert!(rels.contains(&"README.md".to_string()));
        assert!(rels.contains(&"src/main.rs".to_string()));
        // secrets and VCS internals must not be present
        assert!(!rels.iter().any(|r| r == ".env"), "indexed .env: {rels:?}");
        assert!(
            !rels.iter().any(|r| r.starts_with(".git")),
            "descended .git: {rels:?}"
        );
        assert!(
            !rels.iter().any(|r| r.ends_with(".secret")),
            "indexed a nested dotfile: {rels:?}"
        );
    }

    /// #276: the walk itself must drop gitignored junk, not a later stage —
    /// an ignored directory is never descended, so it never reaches hashing.
    #[test]
    fn the_walk_honours_gitignore_and_the_built_in_defaults() {
        use super::walk_reporting;
        use crate::ignore_rules::IgnoreOptions;

        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        fs::write(root.join(".gitignore"), "*.tmp\ndocs/private/\n").unwrap();
        fs::write(root.join("README.md"), "hello").unwrap();
        fs::write(root.join("scratch.tmp"), "junk").unwrap();
        fs::create_dir_all(root.join("docs/private")).unwrap();
        fs::write(root.join("docs/private/salary.md"), "secret").unwrap();
        fs::create_dir_all(root.join("target/debug")).unwrap();
        fs::write(root.join("target/debug/huge.rlib"), "bytes").unwrap();

        let (files, report) = walk_reporting(root, false, IgnoreOptions::default()).unwrap();
        let rels: Vec<String> = files.into_iter().map(|e| e.rel).collect();
        assert_eq!(rels, vec!["README.md".to_string()], "{rels:?}");
        // 2 = scratch.tmp, plus `.gitignore` itself under the (separate,
        // non-optional) hidden-file rule.
        assert_eq!(report.files_skipped, 2, "{report:?}");
        assert_eq!(report.dirs_pruned, 2, "{report:?}");
        assert_eq!(report.ignore_files_read, 1, "{report:?}");

        // --no-ignore brings all of it back, except the dotfiles.
        let (all, off) = walk_reporting(root, false, IgnoreOptions::off()).unwrap();
        let mut rels: Vec<String> = all.into_iter().map(|e| e.rel).collect();
        rels.sort();
        assert_eq!(
            rels,
            vec![
                "README.md".to_string(),
                "docs/private/salary.md".to_string(),
                "scratch.tmp".to_string(),
                "target/debug/huge.rlib".to_string(),
            ],
            "{rels:?}"
        );
        assert_eq!(off.dirs_pruned, 0, "{off:?}");
    }

    /// Create a real checkout at `path`. Returns false when the `git` binary
    /// is unavailable, in which case the caller still gets the on-disk shape
    /// that matters (`.git/HEAD`) but skips the parity cross-check.
    #[cfg(test)]
    fn git_init(path: &std::path::Path) -> bool {
        fs::create_dir_all(path).unwrap();
        let ok = std::process::Command::new("git")
            // A developer's global gitignore or an enclosing repo's config
            // must not decide what this test sees.
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .args(["init", "-q"])
            .current_dir(path)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            fs::create_dir_all(path.join(".git")).unwrap();
            fs::write(path.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();
        }
        ok
    }

    /// Files a repository considers its own and does not ignore, as git itself
    /// reports them. Dotfiles are dropped because XERJ's separate, non-optional
    /// hidden-file rule removes them before any ignore rule is consulted.
    #[cfg(test)]
    fn git_untracked_visible_files(repo: &std::path::Path) -> Vec<String> {
        let out = std::process::Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .args(["status", "--porcelain", "--untracked-files=all"])
            .current_dir(repo)
            .output()
            .expect("git status");
        let mut files: Vec<String> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| l.strip_prefix("?? "))
            .map(|p| p.trim_matches('"').to_string())
            .filter(|p| !p.split('/').any(|c| c.starts_with('.')))
            .collect();
        files.sort();
        files
    }

    /// #279 (follow-up to #276). An outer repository's `.gitignore` must not
    /// judge files inside a nested checkout.
    ///
    /// This is git's authority model, not a nicety: the repository that owns a
    /// file decides whether it is ignored. `git status` in the outer repo does
    /// not descend into a nested checkout at all, and `git status` inside the
    /// nested one never consults the outer's `.gitignore`. The first cut of
    /// this feature walked every layer with no boundary stop, so a root `*.md`
    /// hid the README of every vendored dependency below it.
    ///
    /// The fixture is a real nested checkout — the boundary is a `.git` on
    /// disk, so a matcher-level unit test could not have caught this.
    #[test]
    fn an_outer_repository_does_not_judge_a_nested_checkout() {
        use super::walk_reporting;
        use crate::ignore_rules::IgnoreOptions;

        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().join("outer");
        let have_git = git_init(&root);
        fs::write(root.join(".gitignore"), "*.md\n").unwrap();
        fs::write(root.join("OUTER.md"), "outer, and outer's to ignore").unwrap();
        fs::write(root.join("keep.txt"), "x").unwrap();

        let inner = root.join("vendored");
        git_init(&inner);
        fs::write(inner.join(".gitignore"), "local-junk.txt\n").unwrap();
        fs::write(inner.join("README.md"), "the vendored project's own README").unwrap();
        fs::write(inner.join("local-junk.txt"), "inner's own rule drops this").unwrap();
        fs::create_dir_all(inner.join("src")).unwrap();
        fs::write(inner.join("src").join("guide.md"), "also the inner repo's").unwrap();

        let (files, _) = walk_reporting(&root, false, IgnoreOptions::default()).unwrap();
        let mut rels: Vec<String> = files.into_iter().map(|e| e.rel).collect();
        rels.sort();
        assert_eq!(
            rels,
            vec![
                "keep.txt".to_string(),
                "vendored/README.md".to_string(),
                "vendored/src/guide.md".to_string(),
            ],
            "the outer *.md must stop at the nested checkout, and the inner \
             repo's own rule must still apply inside it: {rels:?}"
        );

        // Parity with the tool whose semantics we claim to reproduce: the set
        // XERJ keeps under `vendored/` is the set the nested repository itself
        // reports as its own, unignored, visible files.
        if have_git {
            let from_git = git_untracked_visible_files(&inner);
            assert_eq!(
                from_git,
                vec!["README.md".to_string(), "src/guide.md".to_string()],
                "fixture drifted from git's own answer"
            );
            let from_xerj: Vec<String> = rels
                .iter()
                .filter_map(|r| r.strip_prefix("vendored/").map(str::to_string))
                .collect();
            assert_eq!(from_xerj, from_git, "XERJ and git must agree");
        }
    }

    /// The other half of the boundary: `.xerjignore` is XERJ's own file, not
    /// git's, and its stated job is to govern the folder you pointed at. It
    /// deliberately keeps its authority across a nested checkout, so one file
    /// at the root can say "not this, anywhere below here".
    #[test]
    fn a_root_xerjignore_still_reaches_into_a_nested_checkout() {
        use super::walk_reporting;
        use crate::ignore_rules::IgnoreOptions;

        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().join("outer");
        git_init(&root);
        fs::write(root.join(".xerjignore"), "*.md\n").unwrap();
        fs::write(root.join("keep.txt"), "x").unwrap();

        let inner = root.join("vendored");
        git_init(&inner);
        fs::write(inner.join("README.md"), "x").unwrap();
        fs::write(inner.join("code.rs"), "x").unwrap();

        let (files, _) = walk_reporting(&root, false, IgnoreOptions::default()).unwrap();
        let mut rels: Vec<String> = files.into_iter().map(|e| e.rel).collect();
        rels.sort();
        assert_eq!(
            rels,
            vec!["keep.txt".to_string(), "vendored/code.rs".to_string()],
            "{rels:?}"
        );
    }

    #[test]
    fn a_brain_over_a_dot_named_root_still_works() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().join(".notes");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("a.md"), "x").unwrap();
        let rels: Vec<String> = walk(&root, false)
            .unwrap()
            .into_iter()
            .map(|e| e.rel)
            .collect();
        assert_eq!(
            rels,
            vec!["a.md".to_string()],
            "root exemption failed: {rels:?}"
        );
    }
}
