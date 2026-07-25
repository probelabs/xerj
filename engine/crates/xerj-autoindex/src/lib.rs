//! `xerj autoindex` — point it at ANY folder and it makes the contents
//! AI-searchable with ZERO configuration. Pure ES-compat HTTP client feature:
//! it does NOT link xerj-engine, works against any endpoint, and cannot
//! destabilize the server.

pub mod catalog;
pub mod cli;
pub mod coerce;
mod content;
pub mod correlate;
pub mod dataset;
pub mod esclient;
pub mod extract;
pub mod ids;
pub mod infer;
pub mod sniff;
pub mod state;
pub mod walk;

use anyhow::{Context, Result};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Seek, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use cli::{Cmd, IndexCfg, MapCfg, StatusCfg};
use esclient::Es;
use sniff::{Family, Sniffed};
use state::{FileAssignment, FileDone, JunkFile, Plan, PlanDataset};

/// Entry point for the `xerj autoindex` subcommand (blocking; the server
/// binary calls this via spawn_blocking). Returns the process exit code.
pub fn run_cli() -> i32 {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let cmd = match cli::parse(args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}\n");
            cli::print_help();
            return 2;
        }
    };
    let res = match cmd {
        Cmd::Help => {
            cli::print_help();
            return 0;
        }
        Cmd::Index(cfg) => run_index(cfg),
        Cmd::Map(cfg) => run_map(cfg),
        Cmd::Status(cfg) => run_status(cfg),
    };
    match res {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            1
        }
    }
}

const GB: u64 = 1 << 30;
const SAMPLE_LIMIT_BYTES: u64 = 4 << 20;
const SQLDUMP_SAMPLE_LIMIT: u64 = 64 << 20;

#[cfg(test)]
static REPLACEMENT_FAILPOINT: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

#[cfg(test)]
fn replacement_failpoint(boundary: u8) -> Result<()> {
    if REPLACEMENT_FAILPOINT
        .compare_exchange(boundary, 0, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        anyhow::bail!("injected replacement crash boundary {boundary}");
    }
    Ok(())
}

#[cfg(not(test))]
#[inline]
fn replacement_failpoint(_boundary: u8) -> Result<()> {
    Ok(())
}

fn record_bulk_outcome(
    es: &Es,
    body: Vec<u8>,
    junk_records: &AtomicU64,
    bulk_errors: &Mutex<Vec<String>>,
    send_err: &mut Option<String>,
) -> bool {
    match es.bulk(body) {
        Ok(outcome) => {
            if outcome.server_errors > 0 {
                *send_err = Some(format!(
                    "bulk backend failed for {} item(s): {}. Source file was not journaled \
                     complete; fix the server/embedding configuration and rerun autoindex",
                    outcome.server_errors,
                    outcome
                        .first_server_error
                        .as_deref()
                        .unwrap_or("unknown server error")
                ));
                return true;
            }
            if outcome.item_errors > 0 {
                junk_records.fetch_add(outcome.item_errors, Ordering::Relaxed);
                if let Some(error) = outcome.first_error {
                    let mut errors = bulk_errors.lock().unwrap();
                    if errors.len() < 5 {
                        errors.push(error);
                    }
                }
            }
            false
        }
        Err(error) => {
            *send_err = Some(format!("{error:#}"));
            true
        }
    }
}

// ─── Phase A: per-file scan (sniff + bounded sampling) ───────────────────

struct FileScan {
    sniffed: Option<Sniffed>,
    /// group → (field accs, sampled record count)
    sketches: Vec<(Option<String>, HashMap<String, infer::FieldAcc>, u64)>,
    junk: Option<(String, String)>, // (status, reason)
}

fn scan_file(path: &Path, size: u64, sample: usize, max_file_gb: u64) -> FileScan {
    let mut out = FileScan {
        sniffed: None,
        sketches: Vec::new(),
        junk: None,
    };
    let sn = match sniff::sniff(path) {
        Ok(s) => s,
        Err(e) => {
            out.junk = Some(("junk".into(), format!("unreadable: {e}")));
            return out;
        }
    };
    if sn.family == Family::Binary {
        out.junk = Some((
            "junk".into(),
            format!(
                "binary content ({})",
                sn.binary_kind.clone().unwrap_or_else(|| "unknown".into())
            ),
        ));
        out.sniffed = Some(sn);
        return out;
    }
    // whole-file families get a size cap; streaming families don't need one
    let whole_file = matches!(
        sn.family,
        Family::Json | Family::Html | Family::Yaml | Family::TxtProse | Family::Pdf | Family::Docx
    );
    if whole_file && size > max_file_gb * GB {
        out.junk = Some((
            "skipped".into(),
            format!(
                "oversized for non-streaming family {} (> {max_file_gb} GB)",
                sn.family.as_str()
            ),
        ));
        out.sniffed = Some(sn);
        return out;
    }
    let limit = match sn.family {
        Family::SqlDump => Some(SQLDUMP_SAMPLE_LIMIT),
        Family::Jsonl | Family::Logs | Family::Csv | Family::TxtLines => Some(SAMPLE_LIMIT_BYTES),
        Family::Sqlite => Some(1), // signals per-table row cap inside the extractor
        _ => None,                 // whole-file extractors cap themselves
    };
    let mut groups: HashMap<Option<String>, (HashMap<String, infer::FieldAcc>, u64)> =
        HashMap::new();
    let grouped_family = matches!(sn.family, Family::SqlDump | Family::Sqlite);
    let mut sink = |rec: extract::RawRecord| -> bool {
        let entry = groups.entry(rec.group.clone()).or_default();
        if (entry.1 as usize) < sample {
            for (k, v) in &rec.fields {
                entry.0.entry(k.clone()).or_default().add(v);
            }
        }
        entry.1 += 1;
        if grouped_family {
            true // read on — later tables still need sampling
        } else {
            (entry.1 as usize) < sample
        }
    };
    match extract::extract(path, &sn, limit, &mut sink) {
        Ok(stats) => {
            if groups.is_empty() {
                out.junk = Some((
                    "junk".into(),
                    format!(
                        "no records extracted ({} candidate family, {} junk lines)",
                        sn.family.as_str(),
                        stats.junk
                    ),
                ));
            }
        }
        Err(e) => {
            if groups.is_empty() {
                out.junk = Some(("junk".into(), format!("extract failed: {e}")));
            }
        }
    }
    out.sketches = groups.into_iter().map(|(g, (f, n))| (g, f, n)).collect();
    out.sketches.sort_by(|a, b| a.0.cmp(&b.0));
    out.sniffed = Some(sn);
    out
}

// ─── mapping builder ─────────────────────────────────────────────────────

pub const PROVENANCE_FIELDS: &[&str] = &[
    "ax_path",
    "ax_paths",
    "ax_file",
    "ax_locator",
    "ax_dataset",
    "ax_run",
    "ax_format",
];

fn build_mapping(specs: &[infer::FieldSpec]) -> Value {
    let mut props = Map::new();
    for s in specs {
        let m = match s.es_type.as_str() {
            "date" => json!({"type": "date", "format": "strict_date_optional_time||epoch_millis"}),
            t => json!({"type": t}),
        };
        props.insert(s.name.clone(), m);
    }
    for p in PROVENANCE_FIELDS {
        props.insert((*p).into(), json!({"type": "keyword"}));
    }
    json!({"mappings": {"properties": props}})
}

// ─── the main run ────────────────────────────────────────────────────────

fn select_resume_plan_keys(
    files: &[walk::FileEntry],
    content_keys: &[String],
    plan: &Plan,
) -> Result<Vec<Option<String>>> {
    let mut planned_by_rel: HashMap<&str, &str> = HashMap::new();
    let mut planned_by_path_id: HashMap<&str, &str> = HashMap::new();
    for (key, assignment) in &plan.files {
        if let Some(previous) = planned_by_rel.insert(&assignment.rel, key) {
            anyhow::bail!(
                "resume plan assigns path {} to both {} and {}; use --fresh after verifying the \
                 existing index",
                assignment.rel,
                previous,
                key
            );
        }
        if !assignment.path_id.is_empty() {
            if let Some(previous) = planned_by_path_id.insert(&assignment.path_id, key) {
                anyhow::bail!(
                    "resume plan assigns one native path identity to both {} and {}; use --fresh \
                     after verifying the existing index",
                    previous,
                    key
                );
            }
        }
    }
    let current_rels: std::collections::HashSet<&str> =
        files.iter().map(|file| file.rel.as_str()).collect();
    let current_path_ids: std::collections::HashSet<&str> =
        files.iter().map(|file| file.rel_id.as_str()).collect();
    let mut claimed = std::collections::HashSet::new();
    let mut selected = Vec::with_capacity(files.len());
    for (file, content_key) in files.iter().zip(content_keys) {
        let exact_path = planned_by_path_id
            .get(file.rel_id.as_str())
            .or_else(|| planned_by_rel.get(file.rel.as_str()))
            .filter(|key| !claimed.contains(**key))
            .map(|key| (*key).to_string());
        let exact_content = plan
            .files
            .contains_key(content_key)
            .then(|| content_key.clone())
            .filter(|key| !claimed.contains(key.as_str()));
        let key = if let Some(key) = exact_path.or(exact_content) {
            Some(key)
        } else {
            // Computing the legacy prefix key is intentionally the final
            // fallback. Normal resumes are O(files), with no 64 KiB read.
            let legacy_key = ids::file_key(&file.path, file.size)?;
            if let Some(assignment) = plan.files.get(&legacy_key) {
                let has_exact_current_owner = current_rels.contains(assignment.rel.as_str())
                    || (!assignment.path_id.is_empty()
                        && current_path_ids.contains(assignment.path_id.as_str()));
                if claimed.contains(legacy_key.as_str()) || has_exact_current_owner {
                    anyhow::bail!(
                        "{} collides with legacy resume key {} already owned by {}. No documents \
                         were changed; rerun with --fresh to build distinct full-content keys",
                        file.rel,
                        legacy_key,
                        assignment.rel
                    );
                }
                Some(legacy_key)
            } else {
                None
            }
        };
        if let Some(key) = &key {
            claimed.insert(key.clone());
        }
        selected.push(key);
    }
    Ok(selected)
}

fn alias_keys_to_reindex(
    previous: &[state::DuplicateFile],
    current: &[state::DuplicateFile],
    migration_keys: Option<&[String]>,
) -> std::collections::HashSet<String> {
    let paths_by_key = |aliases: &[state::DuplicateFile]| {
        let mut by_key: HashMap<String, std::collections::BTreeSet<String>> = HashMap::new();
        for alias in aliases {
            by_key
                .entry(alias.file_key.clone())
                .or_default()
                .insert(alias.rel.clone());
        }
        by_key
    };
    let previous = paths_by_key(previous);
    let current = paths_by_key(current);
    let mut changed = std::collections::HashSet::new();
    if let Some(keys) = migration_keys {
        changed.extend(keys.iter().cloned());
    }
    for key in previous.keys().chain(current.keys()) {
        if previous.get(key) != current.get(key) {
            changed.insert(key.clone());
        }
    }
    changed
}

fn run_index(cfg: IndexCfg) -> Result<i32> {
    extract::pdf::configure_workers(cfg.pdf_workers);
    extract::pdf::configure_timeout(cfg.pdf_timeout_secs);
    use rayon::prelude::*;
    let t0 = Instant::now();
    if !cfg.quiet {
        eprintln!(
            "autoindex: bulk HTTP request timeout: {}s",
            cfg.bulk_timeout_secs
        );
    }
    let es = Es::with_bulk_timeout(&cfg.url, cfg.api_key.clone(), cfg.bulk_timeout_secs)?;
    es.ping()?;

    let discovered_files = walk::walk(&cfg.root, cfg.follow_symlinks)?;
    if discovered_files.is_empty() {
        println!("no files found under {}", cfg.root.display());
        return Ok(0);
    }
    let root_str = cfg
        .root
        .canonicalize()
        .unwrap_or_else(|_| cfg.root.clone())
        .to_string_lossy()
        .to_string();
    if !cfg.quiet {
        eprintln!(
            "autoindex: {} files ({} MB) under {}",
            discovered_files.len(),
            discovered_files.iter().map(|f| f.size).sum::<u64>() / (1 << 20),
            root_str
        );
    }

    let state_dir = cfg
        .state_dir
        .clone()
        .unwrap_or_else(|| state::default_state_dir(&root_str, &cfg.url, &cfg.prefix));
    let mut journal = state::Journal::open(
        &state_dir,
        &root_str,
        &cfg.url,
        &cfg.prefix,
        cfg.bulk_timeout_secs,
        cfg.fresh,
    )?;
    let resumed_with_plan = journal.plan.is_some();
    let run_id = journal.run_id.clone();
    if journal.resumed && !cfg.quiet {
        eprintln!(
            "resuming from journal {} ({} files already done)",
            journal.path().display(),
            journal.done.len()
        );
    }
    // Full hashing on every run is deliberate: size/mtime/inode fingerprints
    // cannot prove byte identity across all supported local and network
    // filesystems. A metadata-only shortcut could leave stale live documents
    // forever after a same-size rewrite with restored or stale timestamps.
    let mut inventory = content::resolve(discovered_files)?;
    let mut content_changed = std::collections::HashSet::new();
    let mut stale_alias_ids = Vec::new();
    let mut alias_paths_to_replace = std::collections::HashSet::new();
    let mut plan_changed = journal.plan.is_none();
    // Preserve legacy document IDs while upgrading old plans with full digests.
    // A later same-size/tail mutation is then detected and reindexed.
    if let Some(plan) = &mut journal.plan {
        let needs_alias_path_migration = !plan.alias_paths_indexed;
        let previous_aliases = plan.duplicate_files.clone();
        alias_paths_to_replace.extend(previous_aliases.iter().map(|alias| alias.rel.clone()));
        stale_alias_ids.extend(
            plan.duplicate_files
                .iter()
                .map(|old| catalog::duplicate_file_id(&old.file_key, &old.rel, &old.path_id)),
        );
        let selected_plan_keys = select_resume_plan_keys(&inventory.files, &inventory.keys, plan)?;
        for (index, planned_key) in selected_plan_keys.into_iter().enumerate() {
            let file = &inventory.files[index];
            if let Some(planned_key) = planned_key {
                let assignment = plan.files.get_mut(&planned_key).expect("planned key");
                if assignment.rel != file.rel {
                    content_changed.insert(planned_key.clone());
                    plan_changed = true;
                }
                if assignment
                    .content_digest
                    .as_deref()
                    .is_some_and(|digest| digest != inventory.digests[index])
                {
                    content_changed.insert(planned_key.clone());
                    plan_changed = true;
                }
                if assignment.path_id != file.rel_id
                    || assignment.content_digest.as_deref()
                        != Some(inventory.digests[index].as_str())
                {
                    plan_changed = true;
                }
                assignment.rel = file.rel.clone();
                assignment.path_id = file.rel_id.clone();
                assignment.content_digest = Some(inventory.digests[index].clone());
                inventory.keys[index] = planned_key;
            }
        }
        let key_by_path: HashMap<&str, &str> = inventory
            .files
            .iter()
            .zip(inventory.keys.iter())
            .map(|(file, key)| (file.rel.as_str(), key.as_str()))
            .collect();
        for duplicate in &mut inventory.duplicates {
            if let Some(key) = key_by_path.get(duplicate.duplicate_of.as_str()) {
                duplicate.file_key = (*key).to_string();
            }
        }
        let current_alias_ids: std::collections::HashSet<String> = inventory
            .duplicates
            .iter()
            .map(|alias| catalog::duplicate_file_id(&alias.file_key, &alias.rel, &alias.path_id))
            .collect();
        stale_alias_ids.retain(|id| !current_alias_ids.contains(id));
        // The historical global flag cannot identify which live documents
        // already carry ax_paths. Its one-time migration must rewrite every
        // canonical key; ordinary alias changes remain scoped per key.
        content_changed.extend(alias_keys_to_reindex(
            &previous_aliases,
            &inventory.duplicates,
            needs_alias_path_migration.then_some(inventory.keys.as_slice()),
        ));
        if needs_alias_path_migration {
            plan.alias_paths_indexed = true;
            plan_changed = true;
        }
        if previous_aliases != inventory.duplicates {
            plan_changed = true;
        }
        plan.duplicate_files = inventory.duplicates.clone();
    }
    alias_paths_to_replace.extend(inventory.duplicates.iter().map(|alias| alias.rel.clone()));
    let files = inventory.files;
    let keys = inventory.keys;
    let digests = inventory.digests;
    let duplicate_files = inventory.duplicates;
    let paths_discovered = files.len() + duplicate_files.len();
    if !duplicate_files.is_empty() && !cfg.quiet {
        eprintln!(
            "autoindex: {} byte-identical duplicate path(s) will reuse canonical content",
            duplicate_files.len()
        );
        for duplicate in duplicate_files.iter().take(10) {
            eprintln!(
                "  duplicate: {} → {}",
                duplicate.rel, duplicate.duplicate_of
            );
        }
        if duplicate_files.len() > 10 {
            eprintln!("  … and {} more", duplicate_files.len() - 10);
        }
    }

    // ── Phase A: inference (skipped when a frozen plan exists) ──────────
    let mut clusters_rt: Option<Vec<dataset::Cluster>> = None;
    let plan: Plan = if let Some(p) = journal.plan.clone() {
        p
    } else {
        if !cfg.quiet {
            eprintln!("phase A: sniffing + sampling {} files…", files.len());
        }
        let scans: Vec<FileScan> = files
            .par_iter()
            .map(|f| scan_file(&f.path, f.size, cfg.sample, cfg.max_file_gb))
            .collect();

        let rels: Vec<String> = files.iter().map(|f| f.rel.clone()).collect();
        let mut sketches = Vec::new();
        let mut junk_files = Vec::new();
        for (i, sc) in scans.into_iter().enumerate() {
            let family = sc
                .sniffed
                .as_ref()
                .map(|s| s.family)
                .unwrap_or(Family::Binary);
            if let Some((status, reason)) = sc.junk {
                junk_files.push(JunkFile {
                    file_key: keys[i].clone(),
                    rel: files[i].rel.clone(),
                    format: format_str(sc.sniffed.as_ref()),
                    status,
                    reason,
                    bytes: files[i].size,
                });
                continue;
            }
            for (group, fields, n) in sc.sketches {
                sketches.push(dataset::Sketch {
                    file_idx: i,
                    group,
                    family,
                    fields,
                    records: n,
                });
            }
        }
        let clusters = dataset::cluster(sketches, &rels);
        if !cfg.quiet {
            eprintln!(
                "phase A: {} datasets inferred, {} junk/skipped files",
                clusters.len(),
                junk_files.len()
            );
        }

        // per-file assignments
        let mut file_assignments: HashMap<String, FileAssignment> = HashMap::new();
        for (ci, c) in clusters.iter().enumerate() {
            for &m in &c.members {
                let key = &keys[m];
                let sn = sniff::sniff(&files[m].path).ok();
                let fa = file_assignments
                    .entry(key.clone())
                    .or_insert_with(|| FileAssignment {
                        rel: files[m].rel.clone(),
                        path_id: files[m].rel_id.clone(),
                        family: c.family.as_str().to_string(),
                        gzip: sn.map(|s| s.gzip).unwrap_or(false),
                        content_digest: Some(digests[m].clone()),
                        assignments: Vec::new(),
                    });
                fa.assignments
                    .push((c.group.clone(), clusters[ci].slug.clone()));
            }
        }

        let mut datasets = Vec::new();
        for c in &clusters {
            let specs = infer::infer_fields(&c.fields, c.records, cfg.no_semantic);
            let time_field = infer::elect_time_field(&specs);
            let semantic_field = specs
                .iter()
                .find(|s| s.es_type == "semantic_text")
                .map(|s| s.name.clone());
            datasets.push(PlanDataset {
                slug: c.slug.clone(),
                index: format!("{}-{}", cfg.prefix, c.slug),
                family: c.family.as_str().to_string(),
                group: c.group.clone(),
                specs,
                time_field,
                semantic_field,
                sampled_records: c.records,
                file_count: c.members.len(),
            });
        }
        let plan = Plan {
            datasets,
            files: file_assignments,
            junk_files,
            duplicate_files,
            alias_paths_indexed: true,
        };
        clusters_rt = Some(clusters);
        plan
    };

    if cfg.dry_run {
        println!("{}", serde_json::to_string_pretty(&plan)?);
        eprintln!("(dry run — nothing indexed)");
        return Ok(0);
    }

    // ── create indices with explicit mappings ────────────────────────────
    for d in &plan.datasets {
        es.ensure_index(&d.index, &build_mapping(&d.specs))
            .with_context(|| format!("create index {}", d.index))?;
        es.update_mapping(
            &d.index,
            &json!({"properties": {"ax_paths": {"type": "keyword"}}}),
        )
        .with_context(|| format!("upgrade alias-path mapping for {}", d.index))?;
    }
    es.ensure_index(catalog::CATALOG_INDEX, &catalog::catalog_mapping())?;
    es.update_mapping(
        catalog::CATALOG_INDEX,
        &json!({"properties": {"duplicate_of": {"type": "keyword"}}}),
    )
    .context("upgrade autoindex catalog mapping for duplicate aliases")?;
    // A replacement transaction starts before the effective new plan is
    // persisted and before live visibility changes. If the process dies at
    // any later boundary, journal replay removes the older file_done and
    // deterministically schedules a delete-before-replace repair.
    let generation_by_key: HashMap<&str, &str> = keys
        .iter()
        .zip(digests.iter())
        .map(|(key, digest)| (key.as_str(), digest.as_str()))
        .collect();
    // Snapshot whether live records may already exist before this run starts
    // any new publication intents. Fresh first publications can skip the
    // delete/refresh round trip; replacements and crash repairs cannot.
    let mut cleanup_required: std::collections::HashSet<String> = journal
        .done
        .keys()
        .chain(journal.pending_replacements.keys())
        .cloned()
        .collect();
    let mut replacements: Vec<&String> = content_changed
        .iter()
        .filter(|key| {
            let desired = generation_by_key.get(key.as_str()).copied();
            journal.done.contains_key(key.as_str())
                || journal
                    .pending_replacements
                    .get(key.as_str())
                    .is_some_and(|pending| Some(pending.as_str()) != desired)
        })
        .collect();
    replacements.sort();
    for key in replacements {
        journal.file_replace_start(
            key,
            generation_by_key
                .get(key.as_str())
                .copied()
                .unwrap_or("unknown"),
        )?;
    }
    // Persist only an effective plan change. Repeating the full plan on every
    // no-op resume caused journal growth proportional to plan_size × runs.
    if plan_changed {
        journal.write_plan(&plan)?;
    }
    replacement_failpoint(1).context("after durable replacement plan")?;

    // ── Phase B: full-stream extraction + bulk indexing ─────────────────
    struct DsRt {
        index: String,
        plan: HashMap<String, coerce::Coerce>,
        records: AtomicU64,
        junk: AtomicU64,
        dropped: AtomicU64,
        bytes: AtomicU64,
    }
    let mut ds_rt: HashMap<String, DsRt> = HashMap::new();
    for d in &plan.datasets {
        ds_rt.insert(
            d.slug.clone(),
            DsRt {
                index: d.index.clone(),
                plan: coerce::plan_from_specs(&d.specs),
                records: AtomicU64::new(0),
                junk: AtomicU64::new(0),
                dropped: AtomicU64::new(0),
                bytes: AtomicU64::new(0),
            },
        );
    }

    let done0 = journal.done_keys();
    let planned_junk: std::collections::HashSet<&str> = plan
        .junk_files
        .iter()
        .map(|j| j.file_key.as_str())
        .collect();
    let mut new_unplanned: Vec<JunkFile> = Vec::new();
    let mut todo: Vec<usize> = Vec::new();
    for i in 0..files.len() {
        if keys[i].is_empty() || done0.contains(&keys[i]) && !content_changed.contains(&keys[i]) {
            continue;
        }
        if plan.files.contains_key(&keys[i]) {
            todo.push(i);
        } else if !planned_junk.contains(keys[i].as_str()) {
            // file appeared after the plan was frozen — recorded, not fatal
            new_unplanned.push(JunkFile {
                file_key: keys[i].clone(),
                rel: files[i].rel.clone(),
                format: "unknown".into(),
                status: "skipped".into(),
                reason: "not in the frozen resume plan (re-run with --fresh to include new files)"
                    .into(),
                bytes: files[i].size,
            });
        }
    }
    if resumed_with_plan {
        // Legacy journals predate intent-before-publication and may have live
        // partial records without either file_done or file_replace_start.
        // Conservatively clean every resumed planned todo once. Only a plan
        // created in this process can prove that its first publication is
        // genuinely fresh and skip the delete round trip.
        cleanup_required.extend(todo.iter().map(|&i| keys[i].clone()));
    }
    // Every publication, including a fresh one, receives durable intent
    // before its first bulk. A failed fresh publication therefore skips the
    // unnecessary delete now but is recognized as pending and cleaned on the
    // next run.
    let mut intent_keys: Vec<&str> = todo.iter().map(|&i| keys[i].as_str()).collect();
    intent_keys.sort_unstable();
    intent_keys.dedup();
    for key in intent_keys {
        let generation = generation_by_key.get(key).copied().unwrap_or("unknown");
        if journal
            .pending_replacements
            .get(key)
            .is_none_or(|pending| pending != generation)
        {
            journal.file_replace_start(key, generation)?;
        }
    }
    // ascending by size — workers pop() from the tail, so the BIGGEST files
    // start first and can't serialize the end of the run.
    todo.sort_by_key(|&i| files[i].size);
    let n_todo = todo.len();
    if !cfg.quiet {
        eprintln!(
            "phase B: indexing {} files with {} workers → {}",
            n_todo, cfg.workers, cfg.url
        );
    }

    let queue = Mutex::new(todo);
    let mut paths_by_key: HashMap<String, Vec<String>> = files
        .iter()
        .zip(keys.iter())
        .map(|(file, key)| (key.clone(), vec![file.rel.clone()]))
        .collect();
    for duplicate in &plan.duplicate_files {
        paths_by_key
            .entry(duplicate.file_key.clone())
            .or_default()
            .push(duplicate.rel.clone());
    }
    for paths in paths_by_key.values_mut() {
        paths.sort();
        paths.dedup();
    }
    let journal_mx = Mutex::new(&mut journal);
    let files_done = AtomicU64::new(0);
    let records_total = AtomicU64::new(0);
    let junk_records = AtomicU64::new(0);
    let bulk_errors = Mutex::new(Vec::<String>::new());
    let extra_junk = Mutex::new(Vec::<JunkFile>::new());
    let bulk_cut = cfg.bulk_mb << 20;

    std::thread::scope(|scope| {
        for _ in 0..cfg.workers.min(n_todo.max(1)) {
            scope.spawn(|| {
                loop {
                    let i = match queue.lock().unwrap().pop() {
                        Some(i) => i,
                        None => break,
                    };
                    let f = &files[i];
                    let key = &keys[i];
                    let expected_digest = &digests[i];
                    let fa = plan.files.get(key).unwrap();
                    let asg: HashMap<Option<String>, String> =
                        fa.assignments.iter().cloned().collect();
                    let sn = match sniff::sniff(&f.path) {
                        Ok(s) => s,
                        Err(e) => {
                            extra_junk.lock().unwrap().push(JunkFile {
                                file_key: key.clone(),
                                rel: f.rel.clone(),
                                format: "unknown".into(),
                                status: "junk".into(),
                                reason: format!("unreadable at index time: {e}"),
                                bytes: f.size,
                            });
                            continue;
                        }
                    };
                    let mut file_records = 0u64;
                    let mut file_junk = 0u64;
                    let mut send_err: Option<String> = None;
                    let mut staged = match tempfile::Builder::new()
                        .prefix(".autoindex-stage-")
                        .tempfile_in(&state_dir)
                    {
                        Ok(file) => file,
                        Err(error) => {
                            let mut errors = bulk_errors.lock().unwrap();
                            if errors.len() < 5 {
                                errors.push(format!(
                                    "create per-file staging area for {}: {error}",
                                    f.rel
                                ));
                            }
                            continue;
                        }
                    };
                    if let Err(error) = content::verify(&f.path, f.size, expected_digest) {
                        let mut errors = bulk_errors.lock().unwrap();
                        if errors.len() < 5 {
                            errors.push(format!("{error:#}"));
                        }
                        continue;
                    }
                    {
                        let mut sink = |rec: extract::RawRecord| -> bool {
                            let Some(slug) = asg.get(&rec.group).or_else(|| asg.get(&None)) else {
                                file_junk += 1;
                                return true;
                            };
                            let Some(rt) = ds_rt.get(slug) else {
                                file_junk += 1;
                                return true;
                            };
                            let mut fields = rec.fields;
                            let dropped = coerce::coerce_record(&mut fields, &rt.plan);
                            if dropped > 0 {
                                rt.dropped.fetch_add(dropped as u64, Ordering::Relaxed);
                            }
                            fields.insert("ax_path".into(), Value::String(f.rel.clone()));
                            fields.insert(
                                "ax_paths".into(),
                                Value::Array(
                                    paths_by_key
                                        .get(key)
                                        .into_iter()
                                        .flatten()
                                        .cloned()
                                        .map(Value::String)
                                        .collect(),
                                ),
                            );
                            fields.insert("ax_file".into(), Value::String(key.clone()));
                            fields.insert("ax_locator".into(), Value::String(rec.locator.clone()));
                            fields.insert("ax_dataset".into(), Value::String(slug.clone()));
                            fields.insert("ax_run".into(), Value::String(run_id.clone()));
                            fields.insert("ax_format".into(), Value::String(format_str(Some(&sn))));
                            let id = ids::doc_id(slug, key, &rec.locator);
                            let action = json!({"index": {"_index": rt.index, "_id": id}});
                            if let Err(error) = writeln!(
                                staged.as_file_mut(),
                                "{}\n{}",
                                action,
                                serde_json::to_string(&Value::Object(fields))
                                    .unwrap_or_else(|_| "{}".into())
                            ) {
                                send_err =
                                    Some(format!("stage extracted records for {}: {error}", f.rel));
                                return false;
                            }
                            rt.records.fetch_add(1, Ordering::Relaxed);
                            file_records += 1;
                            true
                        };
                        let res = extract::extract(&f.path, &sn, None, &mut sink);
                        match res {
                            Ok(stats) => {
                                file_junk += stats.junk;
                            }
                            Err(e) => {
                                send_err = Some(format!("extract {}: {e}", f.rel));
                                extra_junk.lock().unwrap().push(JunkFile {
                                    file_key: key.clone(),
                                    rel: f.rel.clone(),
                                    format: format_str(Some(&sn)),
                                    status: "junk".into(),
                                    reason: format!("extract failed at index time: {e}"),
                                    bytes: f.size,
                                });
                            }
                        }
                    }
                    if send_err.is_none() {
                        if let Err(error) = content::verify(&f.path, f.size, expected_digest) {
                            send_err = Some(format!(
                                "{error:#}; no records from this changing file were made visible"
                            ));
                        }
                    }
                    // Visibility begins only after extraction and the second
                    // full-content verification. Delete-before-replace makes
                    // a retry clean up any partial prior attempt.
                    if send_err.is_none() && cleanup_required.contains(key) {
                        let mut indices: Vec<&str> = fa
                            .assignments
                            .iter()
                            .filter_map(|(_, slug)| ds_rt.get(slug).map(|rt| rt.index.as_str()))
                            .collect();
                        indices.sort_unstable();
                        indices.dedup();
                        for index in indices {
                            if let Err(error) =
                                es.delete_by_query(index, &json!({"term": {"ax_file": key}}))
                            {
                                send_err = Some(format!(
                                    "remove prior records for {} before replacement: {error:#}",
                                    f.rel
                                ));
                                break;
                            }
                        }
                        if send_err.is_none() {
                            if let Err(error) = replacement_failpoint(2) {
                                send_err = Some(format!("{error:#}"));
                            }
                        }
                    }
                    if send_err.is_none() {
                        if let Err(error) = staged.as_file_mut().rewind() {
                            send_err =
                                Some(format!("rewind staged records for {}: {error}", f.rel));
                        }
                    }
                    if send_err.is_none() {
                        let mut reader = BufReader::new(staged.as_file_mut());
                        let mut buf = Vec::with_capacity(bulk_cut + (1 << 20));
                        let mut docs = 0usize;
                        loop {
                            let mut action = Vec::new();
                            match reader.read_until(b'\n', &mut action) {
                                Ok(0) => break,
                                Ok(_) => {}
                                Err(error) => {
                                    send_err =
                                        Some(format!("read staged action for {}: {error}", f.rel));
                                    break;
                                }
                            }
                            let mut document = Vec::new();
                            match reader.read_until(b'\n', &mut document) {
                                Ok(0) => {
                                    send_err = Some(format!(
                                        "staged record for {} ended without a document",
                                        f.rel
                                    ));
                                    break;
                                }
                                Ok(_) => {}
                                Err(error) => {
                                    send_err = Some(format!(
                                        "read staged document for {}: {error}",
                                        f.rel
                                    ));
                                    break;
                                }
                            }
                            buf.extend_from_slice(&action);
                            buf.extend_from_slice(&document);
                            docs += 1;
                            if (buf.len() >= bulk_cut || docs >= 5000)
                                && record_bulk_outcome(
                                    &es,
                                    std::mem::take(&mut buf),
                                    &junk_records,
                                    &bulk_errors,
                                    &mut send_err,
                                )
                            {
                                break;
                            }
                            if buf.is_empty() {
                                docs = 0;
                                buf.reserve(bulk_cut);
                            }
                        }
                        if !buf.is_empty() && send_err.is_none() {
                            record_bulk_outcome(
                                &es,
                                buf,
                                &junk_records,
                                &bulk_errors,
                                &mut send_err,
                            );
                        }
                    }
                    if let Some(e) = send_err {
                        // endpoint trouble: record, do NOT journal file_done
                        let mut be = bulk_errors.lock().unwrap();
                        if be.len() < 5 {
                            be.push(e);
                        }
                        continue;
                    }
                    if let Err(error) = replacement_failpoint(4) {
                        let mut errors = bulk_errors.lock().unwrap();
                        if errors.len() < 5 {
                            errors.push(format!("{error:#}"));
                        }
                        continue;
                    }
                    records_total.fetch_add(file_records, Ordering::Relaxed);
                    junk_records.fetch_add(file_junk, Ordering::Relaxed);
                    if let Some(rt) = fa.assignments.first().and_then(|(_, slug)| ds_rt.get(slug)) {
                        rt.bytes.fetch_add(
                            f.size / fa.assignments.len().max(1) as u64,
                            Ordering::Relaxed,
                        );
                        if file_junk > 0 {
                            rt.junk.fetch_add(file_junk, Ordering::Relaxed);
                        }
                    }
                    let (commit_result, journal_path) = {
                        let mut journal = journal_mx.lock().unwrap();
                        let path = journal.path().display().to_string();
                        let result = journal.file_done(&FileDone {
                            file_key: key.clone(),
                            path: f.rel.clone(),
                            records: file_records,
                            junk: file_junk,
                            bytes: f.size,
                            generation: Some(expected_digest.clone()),
                        });
                        (result, path)
                    };
                    match commit_result {
                        Ok(()) => {}
                        Err(error) => {
                            let mut errors = bulk_errors.lock().unwrap();
                            if errors.len() < 5 {
                                errors.push(format!(
                                    "durably commit completed source {} to {}: {error:#}. \
                                     Live records may be present, but the file remains pending; \
                                     repair journal storage and rerun autoindex",
                                    f.rel, journal_path
                                ));
                            }
                            continue;
                        }
                    }
                    let dn = files_done.fetch_add(1, Ordering::Relaxed) + 1;
                    if !cfg.quiet && (dn.is_multiple_of(200) || f.size > 5 * (1 << 20)) {
                        eprintln!("  [{dn}/{n_todo}] {} ({} records)", f.rel, file_records);
                    }
                }
            });
        }
    });

    let bulk_errs = bulk_errors.into_inner().unwrap();
    if !bulk_errs.is_empty() {
        anyhow::bail!(
            "autoindex stopped with bulk/backend failures: {}. Failed source files were not \
             journaled complete; fix the reported server or embedding configuration and rerun \
             the same command to resume safely",
            bulk_errs.join(" | ")
        );
    }

    // ── finalize: refresh, verify, correlate, catalog ────────────────────
    es.refresh(&format!("{}-*", cfg.prefix)).ok();

    // live per-dataset counts + time ranges (every claim traces to a run)
    let mut ds_counts: HashMap<String, u64> = HashMap::new();
    let mut ds_timerange: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
    for d in &plan.datasets {
        let cnt = es.count(&d.index).unwrap_or(0);
        ds_counts.insert(d.slug.clone(), cnt);
        if let Some(t) = &d.time_field {
            let body = json!({"size":0,"aggs":{
                "mn":{"min":{"field":t}},"mx":{"max":{"field":t}}}});
            if let Ok(v) = es.search(&d.index, &body) {
                let get = |k: &str| -> Option<String> {
                    let a = v.pointer(&format!("/aggregations/{k}"))?;
                    a.get("value_as_string")
                        .and_then(|s| s.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| {
                            a.get("value").and_then(|f| f.as_f64()).and_then(|ms| {
                                chrono::DateTime::from_timestamp_millis(ms as i64)
                                    .map(|d| infer::dates::to_rfc3339_millis(&d))
                            })
                        })
                };
                ds_timerange.insert(d.slug.clone(), (get("mn"), get("mx")));
            }
        }
    }

    // correlations
    let mut key_corrs: Vec<correlate::KeyCorr> = Vec::new();
    if let Some(clusters) = &clusters_rt {
        let mut cands = Vec::new();
        for (c, d) in clusters.iter().zip(plan.datasets.iter()) {
            for spec in &d.specs {
                let Some(acc) = c.fields.get(&spec.name) else {
                    continue;
                };
                if correlate::is_candidate(
                    &spec.es_type,
                    spec.semantic.as_deref(),
                    acc.distinct.len(),
                    acc.n,
                    acc.distinct_overflow,
                    (acc.long_ok > 0).then_some((acc.int_min, acc.int_max)),
                ) {
                    cands.push(correlate::Candidate {
                        slug: d.slug.clone(),
                        index: d.index.clone(),
                        field: spec.name.clone(),
                        kind: spec.es_type.clone(),
                        values: acc.raw_values.clone(),
                        sampled_n: acc.n,
                    });
                }
            }
        }
        key_corrs = correlate::key_overlaps(&cands);
        for c in key_corrs.iter_mut() {
            correlate::confirm(&es, c, 20).ok();
        }
        // keep only live-confirmed overlaps in the report
        key_corrs.retain(|c| c.confirmed.map(|(n, _)| n > 0).unwrap_or(false));
    } else if !cfg.quiet {
        eprintln!("(resumed run: key-overlap correlations kept from the original run's catalog)");
    }

    let mut series = Vec::new();
    for d in &plan.datasets {
        if let Some(t) = &d.time_field {
            if let Ok(Some(s)) = correlate::fetch_histogram(&es, &d.slug, &d.index, t) {
                series.push(s);
            }
        }
    }
    let time_corrs = correlate::time_alignment(&series);

    // ── catalog write ────────────────────────────────────────────────────
    // Alias IDs changed as identity evolved. Remove by logical path first so
    // catalogs created by any previous identity scheme cannot survive beside
    // the one current alias document.
    for path in &alias_paths_to_replace {
        es.delete_by_query(
            catalog::CATALOG_INDEX,
            &json!({
                "bool": {
                    "filter": [
                        {"term": {"status": "duplicate"}},
                        {"term": {"path": path}}
                    ]
                }
            }),
        )
        .with_context(|| format!("replace catalog alias for {path}"))?;
    }
    let mut cat_buf: Vec<u8> = Vec::new();
    let push_doc = |id: &str, doc: &Value, buf: &mut Vec<u8>| {
        let action = json!({"index": {"_index": catalog::CATALOG_INDEX, "_id": id}});
        buf.extend_from_slice(action.to_string().as_bytes());
        buf.push(b'\n');
        buf.extend_from_slice(doc.to_string().as_bytes());
        buf.push(b'\n');
    };
    for id in &stale_alias_ids {
        let action = json!({"delete": {"_index": catalog::CATALOG_INDEX, "_id": id}});
        cat_buf.extend_from_slice(action.to_string().as_bytes());
        cat_buf.push(b'\n');
    }

    // dataset docs
    let mut junk_records_by_run: u64 = junk_records.load(Ordering::Relaxed);
    for d in &plan.datasets {
        let rt = &ds_rt[&d.slug];
        let sample_queries = catalog::build_sample_queries(d, &key_corrs);
        let mut notes = Vec::new();
        let dropped = rt.dropped.load(Ordering::Relaxed);
        if dropped > 0 {
            notes.push(format!(
                "{dropped} field values could not be coerced to the inferred types and were dropped (records still indexed)"
            ));
        }
        if let Some(g) = &d.group {
            notes.push(format!("source table: {g}"));
        }
        for s in &d.specs {
            for n in &s.notes {
                notes.push(format!("{}: {}", s.name, n));
            }
        }
        // formats incl gz flag
        let mut formats: Vec<String> = plan
            .files
            .values()
            .filter(|fa| fa.assignments.iter().any(|(_, s)| s == &d.slug))
            .map(|fa| {
                if fa.gzip {
                    format!("{}(gzip)", fa.family)
                } else {
                    fa.family.clone()
                }
            })
            .collect();
        formats.sort();
        formats.dedup();
        let (tmin, tmax) = ds_timerange.get(&d.slug).cloned().unwrap_or((None, None));
        let (id, doc) = catalog::dataset_doc(&catalog::DatasetDocInput {
            pd: d,
            record_count: *ds_counts.get(&d.slug).unwrap_or(&0),
            junk_records: rt.junk.load(Ordering::Relaxed),
            bytes: rt.bytes.load(Ordering::Relaxed),
            file_count: d.file_count,
            formats,
            time_min: tmin,
            time_max: tmax,
            sample_queries,
            notes,
            run_id: &run_id,
        });
        push_doc(&id, &doc, &mut cat_buf);
    }

    // file docs — indexed (from journal) + junk/skipped (from plan + this run)
    {
        let j = journal_mx.lock().unwrap();
        for fd in j.done.values() {
            let current_path = plan
                .files
                .get(&fd.file_key)
                .map(|assignment| assignment.rel.as_str())
                .unwrap_or(&fd.path);
            let fmt = plan
                .files
                .get(&fd.file_key)
                .map(|fa| {
                    if fa.gzip {
                        format!("{}(gzip)", fa.family)
                    } else {
                        fa.family.clone()
                    }
                })
                .unwrap_or_else(|| "unknown".into());
            let (id, doc) = catalog::file_doc(
                &fd.file_key,
                current_path,
                &fmt,
                "indexed",
                None,
                fd.records,
                fd.junk,
                fd.bytes,
                &run_id,
            );
            push_doc(&id, &doc, &mut cat_buf);
        }
    }
    let extra = extra_junk.into_inner().unwrap();
    let mut all_junk: Vec<&JunkFile> = plan.junk_files.iter().collect();
    all_junk.extend(extra.iter());
    all_junk.extend(new_unplanned.iter());
    for jf in &all_junk {
        let (id, doc) = catalog::file_doc(
            &jf.file_key,
            &jf.rel,
            &jf.format,
            &jf.status,
            Some(&jf.reason),
            0,
            0,
            jf.bytes,
            &run_id,
        );
        push_doc(&id, &doc, &mut cat_buf);
        junk_records_by_run += 0; // junk FILES tracked separately from junk records
    }
    for duplicate in &plan.duplicate_files {
        let (id, doc) = catalog::duplicate_file_doc(
            &duplicate.file_key,
            &duplicate.rel,
            &duplicate.path_id,
            &duplicate.duplicate_of,
            duplicate.bytes,
            &run_id,
        );
        push_doc(&id, &doc, &mut cat_buf);
    }

    for c in &key_corrs {
        let mut v = c.to_value();
        v["run_id"] = json!(run_id);
        push_doc(&c.id(), &v, &mut cat_buf);
    }
    for (i, tc) in time_corrs.iter().enumerate() {
        let id = format!(
            "tcorr:{}:{}",
            tc.get("a_dataset").and_then(|v| v.as_str()).unwrap_or(""),
            tc.get("b_dataset")
                .and_then(|v| v.as_str())
                .unwrap_or(&i.to_string())
        );
        let mut v = tc.clone();
        v["run_id"] = json!(run_id);
        push_doc(&id, &v, &mut cat_buf);
    }

    let wall = t0.elapsed().as_secs_f64();
    let total_records: u64 = ds_counts.values().sum();
    let run_doc = json!({
        "doc_kind": "run",
        "run_id": run_id,
        "root": root_str,
        "url": cfg.url,
        "prefix": cfg.prefix,
        "started": chrono::Utc::now().to_rfc3339(),
        "files_total": paths_discovered,
        "unique_content_files": files.len(),
        "files_indexed": journal_mx.lock().unwrap().done.len(),
        "duplicate_files": plan.duplicate_files.len(),
        "files_junk": all_junk.len(),
        "records_total": total_records,
        "junk_records_total": junk_records_by_run,
        "wall_seconds": (wall * 10.0).round() / 10.0,
        "workers": cfg.workers,
        "semantic": !cfg.no_semantic,
    });
    push_doc(&format!("run:{run_id}"), &run_doc, &mut cat_buf);

    if !cat_buf.is_empty() {
        es.bulk(cat_buf).context("write catalog")?;
    }
    es.refresh(catalog::CATALOG_INDEX).ok();
    journal_mx.lock().unwrap().finish(&run_doc)?;

    // ── summary ──────────────────────────────────────────────────────────
    let junk_total_records = junk_records.load(Ordering::Relaxed);
    if cfg.json {
        println!("{run_doc}");
    } else if !cfg.quiet {
        println!("\ndone in {wall:.1}s — {} datasets, {} records live, {} duplicate aliases, {} junk records, {} junk/skipped files",
            plan.datasets.len(), total_records, plan.duplicate_files.len(), junk_total_records, all_junk.len());
        let mut rows: Vec<(&String, u64)> = plan
            .datasets
            .iter()
            .map(|d| (&d.index, *ds_counts.get(&d.slug).unwrap_or(&0)))
            .collect();
        rows.sort_by_key(|r| std::cmp::Reverse(r.1));
        for (idx, cnt) in rows {
            println!("  {idx:<40} {cnt:>10} docs");
        }
        println!(
            "\nnext: `xerj autoindex map --url {}` for the data map; search via GET /{}-*/_search",
            cfg.url, cfg.prefix
        );
    }
    Ok(if junk_total_records > 0 || !all_junk.is_empty() {
        3
    } else {
        0
    })
}

fn format_str(sn: Option<&Sniffed>) -> String {
    match sn {
        Some(s) if s.gzip => format!("{}(gzip)", s.family.as_str()),
        Some(s) => s.family.as_str().to_string(),
        None => "unknown".into(),
    }
}

// ─── map subcommand ──────────────────────────────────────────────────────

fn run_map(cfg: MapCfg) -> Result<i32> {
    let es = Es::new(&cfg.url, cfg.api_key.clone())?;
    es.ping()?;
    let fetch = |query: Value, size: usize, sort: Option<Value>| -> Result<Vec<Value>> {
        let mut body = json!({"query": query, "size": size});
        if let Some(s) = sort {
            body["sort"] = s;
        }
        let v = es.search(catalog::CATALOG_INDEX, &body)?;
        Ok(v.pointer("/hits/hits")
            .and_then(|h| h.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|h| h.get("_source").cloned())
                    .collect()
            })
            .unwrap_or_default())
    };
    let mut ds_query = json!({"term": {"doc_kind": "dataset"}});
    if let Some(slug) = &cfg.dataset {
        ds_query = json!({"bool": {"must": [
            {"term": {"doc_kind": "dataset"}},
            {"term": {"slug": slug}}
        ]}});
    }
    let datasets = fetch(ds_query, 500, Some(json!([{"record_count": "desc"}])))?;
    if datasets.is_empty() {
        eprintln!(
            "no autoindex catalog found at {} (index {}) — run `xerj autoindex <folder>` first",
            cfg.url,
            catalog::CATALOG_INDEX
        );
        return Ok(1);
    }
    let mut runs = fetch(json!({"term": {"doc_kind": "run"}}), 50, None)?;
    runs.sort_by_key(|r| {
        std::cmp::Reverse(
            r.get("started")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
        )
    });
    let correlations = {
        let mut all = fetch(json!({"term": {"doc_kind": "correlation"}}), 200, None)?;
        // stale-correlation hygiene: catalog docs upsert by deterministic id,
        // so older runs' correlations linger — show only the latest run that
        // produced each corr_kind.
        for kind in ["key_overlap", "time_alignment"] {
            let latest = all
                .iter()
                .filter(|c| c.get("corr_kind").and_then(|k| k.as_str()) == Some(kind))
                .filter_map(|c| c.get("run_id").and_then(|r| r.as_str()))
                .max()
                .map(|s| s.to_string());
            all.retain(|c| {
                c.get("corr_kind").and_then(|k| k.as_str()) != Some(kind)
                    || c.get("run_id")
                        .and_then(|r| r.as_str())
                        .map(|s| s.to_string())
                        == latest
            });
        }
        all
    };
    let latest_run_filter = runs
        .first()
        .and_then(|run| run.get("run_id"))
        .and_then(|value| value.as_str())
        .map(|run_id| json!({"term": {"run_id": run_id}}));
    let mut junk_must = vec![json!({"term": {"doc_kind": "file"}})];
    if let Some(filter) = latest_run_filter.clone() {
        junk_must.push(filter);
    }
    let junk_files = fetch(
        json!({"bool": {"must": junk_must,
            "must_not": [
                {"term": {"status": "indexed"}},
                {"term": {"status": "duplicate"}}
        ]}}),
        500,
        None,
    )?;
    let mut duplicate_must = vec![
        json!({"term": {"doc_kind": "file"}}),
        json!({"term": {"status": "duplicate"}}),
    ];
    if let Some(filter) = latest_run_filter {
        duplicate_must.push(filter);
    }
    let duplicate_files = fetch(json!({"bool": {"must": duplicate_must}}), 500, None)?;
    if cfg.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "run": runs.first(),
                "datasets": datasets,
                "correlations": correlations,
                "junk_files": junk_files,
                "duplicate_files": duplicate_files,
                "gotchas": catalog::GOTCHAS,
            }))?
        );
    } else {
        print!(
            "{}",
            catalog::render_map(
                runs.first(),
                &datasets,
                &correlations,
                &junk_files,
                &duplicate_files,
                junk_files.len() as u64
            )
        );
    }
    Ok(0)
}

// ─── status subcommand ───────────────────────────────────────────────────

fn run_status(cfg: StatusCfg) -> Result<i32> {
    // journals
    let dirs: Vec<std::path::PathBuf> = match &cfg.state_dir {
        Some(d) => vec![d.clone()],
        None => {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            let base = Path::new(&home).join(".xerj").join("autoindex");
            std::fs::read_dir(&base)
                .map(|rd| rd.flatten().map(|e| e.path()).collect())
                .unwrap_or_default()
        }
    };
    for d in dirs {
        let jp = d.join("journal.ndjson");
        if !jp.exists() {
            continue;
        }
        let mut root = String::new();
        let mut done = 0u64;
        let mut records = 0u64;
        let mut finished = false;
        if let Ok(f) = std::fs::File::open(&jp) {
            use std::io::BufRead;
            for line in std::io::BufReader::new(f).lines().map_while(|l| l.ok()) {
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
                    match v.get("kind").and_then(|k| k.as_str()) {
                        Some("run") => {
                            root = v.get("root").and_then(|r| r.as_str()).unwrap_or("").into()
                        }
                        Some("file_done") => {
                            done += 1;
                            records += v.get("records").and_then(|r| r.as_u64()).unwrap_or(0);
                        }
                        Some("finish") => finished = true,
                        _ => {}
                    }
                }
            }
        }
        println!(
            "journal {} — root {} — {} files done, {} records, {}",
            jp.display(),
            root,
            done,
            records,
            if finished { "FINISHED" } else { "in progress" }
        );
    }
    // live indices
    if let Ok(es) = Es::new(&cfg.url, cfg.api_key.clone()) {
        if es.ping().is_ok() {
            let pat = format!("{}-", cfg.prefix);
            println!("\nlive indices at {}:", cfg.url);
            for (name, docs) in es.cat_indices().unwrap_or_default() {
                if name.starts_with(&pat) || name == catalog::CATALOG_INDEX {
                    println!("  {name:<40} {docs:>10} docs");
                }
            }
        }
    }
    Ok(0)
}

#[cfg(test)]
mod duplicate_integration_tests {
    use super::*;
    use std::collections::HashSet;
    use std::fs;
    use std::io::Write;

    fn legacy_assignment(rel: &str) -> FileAssignment {
        FileAssignment {
            rel: rel.to_string(),
            path_id: String::new(),
            family: "txt".to_string(),
            gzip: false,
            content_digest: None,
            assignments: vec![(None, "text".to_string())],
        }
    }

    #[test]
    fn legacy_prefix_collision_has_one_deterministic_owner() {
        let corpus = tempfile::tempdir().unwrap();
        let mut a = vec![b'x'; 65_537];
        let mut b = a.clone();
        a[65_536] = b'a';
        b[65_536] = b'b';
        fs::write(corpus.path().join("a.txt"), a).unwrap();
        fs::write(corpus.path().join("b.txt"), b).unwrap();
        let files = walk::walk(corpus.path(), false).unwrap();
        let inventory = content::resolve(files.clone()).unwrap();
        let legacy = ids::file_key(&files[0].path, files[0].size).unwrap();
        assert_eq!(
            legacy,
            ids::file_key(&files[1].path, files[1].size).unwrap()
        );

        // The exact historical owner sorts second. It must retain the legacy
        // key; the earlier collision sibling must never steal or share it.
        let mut plan = Plan::default();
        plan.files
            .insert(legacy.clone(), legacy_assignment("b.txt"));
        let error = select_resume_plan_keys(&inventory.files, &inventory.keys, &plan).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("collides with legacy resume key"));
        assert!(message.contains("rerun with --fresh"));
    }

    #[test]
    fn one_alias_change_invalidates_only_its_content_key() {
        let alias = |file_key: &str, rel: &str| state::DuplicateFile {
            file_key: file_key.to_string(),
            rel: rel.to_string(),
            path_id: format!("id:{rel}"),
            duplicate_of: format!("{file_key}.txt"),
            bytes: 10,
        };
        let previous = vec![alias("a", "a-copy.txt"), alias("c", "c-copy.txt")];
        let current = vec![
            alias("a", "a-copy.txt"),
            alias("b", "b-copy.txt"),
            alias("c", "c-copy.txt"),
        ];
        assert_eq!(
            alias_keys_to_reindex(&previous, &current, None),
            HashSet::from(["b".to_string()])
        );
        assert_eq!(
            alias_keys_to_reindex(&current, &previous, None),
            HashSet::from(["b".to_string()])
        );
        assert_eq!(
            alias_keys_to_reindex(
                &previous,
                &current,
                Some(&["a".to_string(), "b".to_string(), "c".to_string()])
            ),
            HashSet::from(["a".to_string(), "b".to_string(), "c".to_string()])
        );
    }

    #[test]
    fn duplicate_content_keeps_journal_and_live_id_cardinality_equal_on_resume() {
        let corpus = tempfile::tempdir().unwrap();
        let state_dir = tempfile::tempdir().unwrap();
        let body = "quarterly revenue was 42\noperating income was 7\n";
        fs::write(corpus.path().join("report-original.txt"), body).unwrap();
        fs::write(corpus.path().join("report-copy.txt"), body).unwrap();

        let discovered = walk::walk(corpus.path(), false).unwrap();
        let inventory = content::resolve(discovered).unwrap();
        assert_eq!(inventory.files.len(), 1);
        assert_eq!(inventory.duplicates.len(), 1);

        let mut live_ids = HashSet::new();
        let mut records = 0u64;
        let sniffed = sniff::sniff(&inventory.files[0].path).unwrap();
        extract::extract(&inventory.files[0].path, &sniffed, None, &mut |record| {
            live_ids.insert(ids::doc_id("text", &inventory.keys[0], &record.locator));
            records += 1;
            true
        })
        .unwrap();

        let mut journal = state::Journal::open(
            state_dir.path(),
            "corpus",
            "http://engine",
            "test",
            300,
            false,
        )
        .unwrap();
        journal
            .write_plan(&Plan {
                duplicate_files: inventory.duplicates.clone(),
                ..Plan::default()
            })
            .unwrap();
        journal
            .file_done(&FileDone {
                file_key: inventory.keys[0].clone(),
                path: inventory.files[0].rel.clone(),
                records,
                junk: 0,
                bytes: inventory.files[0].size,
                generation: Some(inventory.digests[0].clone()),
            })
            .unwrap();
        drop(journal);

        let resumed = state::Journal::open(
            state_dir.path(),
            "corpus",
            "http://engine",
            "test",
            300,
            false,
        )
        .unwrap();
        assert!(resumed.resumed);
        assert_eq!(
            resumed.done.values().map(|f| f.records).sum::<u64>(),
            records
        );
        assert_eq!(records as usize, live_ids.len());
        let done = resumed.done_keys();
        assert!(inventory.keys.iter().all(|key| done.contains(key)));
        let aliases = &resumed.plan.unwrap().duplicate_files;
        assert_eq!(aliases, &inventory.duplicates);
    }

    #[test]
    fn mutation_after_more_than_one_bulk_is_staged_and_retry_replaces_stale_locators() {
        let corpus = tempfile::tempdir().unwrap();
        let path = corpus.path().join("large.csv");
        let mut csv = String::from("id,value\n");
        for id in 0..6_001 {
            csv.push_str(&format!("{id},old-{id}\n"));
        }
        fs::write(&path, csv).unwrap();

        let inventory = content::resolve(walk::walk(corpus.path(), false).unwrap()).unwrap();
        let expected_size = inventory.files[0].size;
        let expected_digest = inventory.digests[0].clone();
        let sniffed = sniff::sniff(&path).unwrap();
        let mut staged = tempfile::NamedTempFile::new().unwrap();
        let mut staged_docs = 0usize;
        extract::extract(&path, &sniffed, None, &mut |record| {
            writeln!(
                staged,
                "{}\n{}",
                record.locator,
                Value::Object(record.fields)
            )
            .unwrap();
            staged_docs += 1;
            if staged_docs == 5_001 {
                // A shorter source replaces the file while extraction is in
                // progress, after the production 5,000-document bulk cut.
                fs::write(&path, "id,value\n0,new-0\n1,new-1\n").unwrap();
            }
            true
        })
        .unwrap();
        assert!(staged_docs > 5_000);

        let mut live: HashSet<String> = (0..6_001).map(|id| format!("row:{id}")).collect();
        assert!(content::verify(&path, expected_size, &expected_digest).is_err());
        // Verification precedes delete/visibility, so a rejected attempt has
        // not mixed any staged records into the old live set.
        assert_eq!(live.len(), 6_001);

        // The retry's delete-before-replace removes every old locator before
        // the now-short source becomes visible.
        live.clear();
        let retry_sniffed = sniff::sniff(&path).unwrap();
        extract::extract(&path, &retry_sniffed, None, &mut |record| {
            live.insert(record.locator);
            true
        })
        .unwrap();
        assert_eq!(live, HashSet::from(["r0".into(), "r1".into()]));
    }
}

#[cfg(test)]
mod failure_resume_http_tests;
