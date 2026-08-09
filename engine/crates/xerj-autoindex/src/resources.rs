//! What one autoindex run may take from the machine it is running on.
//!
//! autoindex runs on someone's laptop while they are using it, so this is the
//! politest consumer XERJ has — and until #240 it was the least considered:
//! `--workers` defaulted to `min(cores, 8)` for no recorded reason, and it did
//! not control phase A at all. Phase A (content hashing, sniffing, sampling)
//! is the CPU-bound half, and it ran on rayon's *default global pool*, i.e.
//! every core, whatever the user asked for. Lowering `--workers` to get a Mac
//! back therefore did nothing, which is what people reported.
//!
//! [`plan`] is the whole policy, as one pure function:
//!
//! * **Phase A** — CPU-bound, latency-critical for the person watching it. Gets
//!   the requested worker count, defaulting to every core
//!   ([`xerj_common::resource::Workload::Latency`]), and now actually runs
//!   inside a pool of that width ([`crate::pool`]).
//! * **Phase B** — network-bound: measured at 0.69 client cores with 32
//!   workers, so cores are not the constraint there; memory is. Each worker
//!   holds a bulk buffer plus the batch it is building, so the count is capped
//!   by the memory safe zone.
//! * **PDF workers** — separate processes, each allowed 1536 MiB of address
//!   space (`extract::pdf::WORKER_ADDRESS_SPACE`), so the cap is whatever the
//!   safe zone can actually pay for, never more than the measured 4.

use xerj_common::resource;

const MIB: u64 = 1024 * 1024;

/// Hard ceiling on `--workers`. A value past this is a typo, and silently
/// clamping typos is the accepted-and-ignored class from #204 — the CLI
/// rejects it instead.
pub const MAX_WORKERS: usize = 1024;

/// Ceiling on concurrent PDF subprocesses, unchanged from the value introduced
/// with the isolated-PDF-worker design: four × 1536 MiB of address space is
/// already the most a laptop should ever hand to one file format.
pub const MAX_PDF_WORKERS: usize = 4;

/// Address space one isolated PDF worker may take, mirroring
/// `extract::pdf::WORKER_ADDRESS_SPACE`. Used only to decide how many such
/// workers the machine can afford.
const PDF_WORKER_BYTES: u64 = 1536 * MIB;

/// Working memory one phase-B worker needs: the NDJSON bulk buffer it fills,
/// the batch of documents it is building, and the request/response copy in
/// flight. Three times the bulk size is an engineering budget, not a
/// measurement — it exists so that `--bulk-mb 24` on a 4 GB machine reduces
/// worker count instead of swapping.
const WORKER_BYTES_PER_BULK_MIB: u64 = 3;

/// The resource plan for one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Threads for the phase-A scan pool (hash, sniff, sample).
    pub scan_threads: usize,
    /// Concurrent phase-B index workers.
    pub index_workers: usize,
    /// Concurrent isolated PDF extraction subprocesses.
    pub pdf_workers: usize,
    /// Bytes this run considers itself entitled to.
    pub safe_zone_bytes: u64,
    /// Anything the machine forced on the run, for the operator to see.
    pub notes: Vec<String>,
}

/// Build the plan for this machine.
///
/// `requested_workers` / `requested_pdf_workers` are the CLI values when the
/// user passed them, `None` when they did not.
pub fn plan(
    requested_workers: Option<usize>,
    requested_pdf_workers: Option<usize>,
    bulk_mb: usize,
) -> Plan {
    plan_for(
        resource::cores(),
        resource::memory_safe_zone_bytes(),
        requested_workers,
        requested_pdf_workers,
        bulk_mb,
    )
}

/// Pure planning rule — the tested half of [`plan`].
pub fn plan_for(
    cores: usize,
    safe_zone_bytes: u64,
    requested_workers: Option<usize>,
    requested_pdf_workers: Option<usize>,
    bulk_mb: usize,
) -> Plan {
    let cores = cores.max(1);
    let mut notes = Vec::new();

    // Phase A: every core by default. "Use the machine" belongs here — this is
    // the phase that is actually CPU-bound.
    let scan_threads = requested_workers.unwrap_or(cores).max(1);

    // Phase B: the same count, unless the safe zone cannot pay for that many
    // in-flight bulk buffers.
    let per_worker = (bulk_mb.max(1) as u64)
        .saturating_mul(WORKER_BYTES_PER_BULK_MIB)
        .saturating_mul(MIB);
    let affordable = (safe_zone_bytes / per_worker).max(1) as usize;
    let mut index_workers = scan_threads;
    if index_workers > affordable {
        notes.push(format!(
            "memory safe zone {} MiB allows {affordable} index workers at --bulk-mb {bulk_mb}, \
             not {index_workers}",
            safe_zone_bytes / MIB
        ));
        index_workers = affordable;
    }

    // PDF workers: bounded by the measured ceiling, by the cores available,
    // and by what the safe zone can pay for at 1536 MiB of address space each.
    let pdf_affordable = (safe_zone_bytes / PDF_WORKER_BYTES).max(1) as usize;
    let pdf_default = cores.min(MAX_PDF_WORKERS);
    let mut pdf_workers = requested_pdf_workers.unwrap_or(pdf_default).max(1);
    if pdf_workers > pdf_affordable {
        notes.push(format!(
            "memory safe zone {} MiB allows {pdf_affordable} PDF worker(s) at 1536 MiB each, \
             not {pdf_workers}",
            safe_zone_bytes / MIB
        ));
        pdf_workers = pdf_affordable;
    }

    Plan {
        scan_threads,
        index_workers,
        pdf_workers: pdf_workers.min(MAX_PDF_WORKERS),
        safe_zone_bytes,
        notes,
    }
}

impl Plan {
    /// One line for the run header and the `--json` summary.
    pub fn describe(&self) -> String {
        format!(
            "resources: {} scan threads, {} index workers, {} pdf workers, memory safe zone {} MiB ({})",
            self.scan_threads,
            self.index_workers,
            self.pdf_workers,
            self.safe_zone_bytes / MIB,
            resource::describe(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * MIB;

    #[test]
    fn the_default_plan_uses_the_whole_machine_for_the_cpu_bound_phase() {
        let plan = plan_for(12, 8 * GIB, None, None, 8);
        assert_eq!(plan.scan_threads, 12, "phase A is the CPU-bound phase");
        assert_eq!(plan.index_workers, 12);
        assert_eq!(plan.pdf_workers, 4);
        assert!(plan.notes.is_empty());
        // The pre-#240 default capped both at 8 on any machine with more cores.
        assert_eq!(plan_for(32, 32 * GIB, None, None, 8).scan_threads, 32);
    }

    #[test]
    fn an_explicit_request_is_honoured_on_both_phases() {
        let plan = plan_for(32, 32 * GIB, Some(2), Some(1), 8);
        assert_eq!(plan.scan_threads, 2, "--workers must bound phase A too");
        assert_eq!(plan.index_workers, 2);
        assert_eq!(plan.pdf_workers, 1);
    }

    #[test]
    fn a_small_machine_gets_fewer_workers_and_is_told_why() {
        // A 32-core box squeezed into a 512 MiB safe zone (small container),
        // asked for the biggest bulk size: 72 MiB per worker buys 7 of them.
        let plan = plan_for(32, 512 * MIB, None, None, 24);
        assert_eq!(plan.index_workers, 7);
        assert_eq!(
            plan.scan_threads, 32,
            "scanning is not what costs the memory"
        );
        assert!(plan.notes[0].contains("index workers"));
        // One PDF worker is all a 1 GiB safe zone can pay for at 1536 MiB each.
        assert_eq!(plan.pdf_workers, 1);
        assert!(plan.notes.iter().any(|n| n.contains("PDF worker")));
    }

    #[test]
    fn the_plan_is_never_zero_or_absurd() {
        for cores in [0usize, 1, 2, 256] {
            for safe in [0u64, 64 * MIB, GIB, 512 * GIB] {
                for bulk in [1usize, 8, 24] {
                    let plan = plan_for(cores, safe, None, None, bulk);
                    assert!(plan.scan_threads >= 1);
                    assert!(plan.index_workers >= 1);
                    assert!((1..=MAX_PDF_WORKERS).contains(&plan.pdf_workers));
                }
            }
        }
    }
}
