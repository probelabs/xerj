//! Single-node WAL tap — push a filtered subset of indices to an external
//! ES-compatible target (issue #320).
//!
//! ## What this is
//!
//! A background loop that tails each allowlisted index's write-ahead log and
//! replays the entries it finds into `{target}/_bulk`. The target can be an
//! Elasticsearch cluster, an OpenSearch cluster, or another xerj node — the
//! wire format is `_bulk` and nothing else, so there is nothing to install on
//! the far side.
//!
//! ## What this is not
//!
//! It is not cross-cluster replication. Nothing flows back, there is no
//! follower state machine, no leader election, no retention leases, and it
//! does not depend on the post-GA multi-node track. `_ccr/*` still answers
//! `501`, honestly, because none of that exists.
//!
//! ## Who may call it
//!
//! Every `/_xerj/*` route is **superuser-only**, enforced in
//! [`crate::authz`](../../xerj_api/authz/index.html) on the first path
//! segment. `PUT /_xerj/wal_tap` names an arbitrary host in `target_url` and
//! an arbitrary index set in `indices`, and attaches an operator-supplied
//! `Authorization` header to what it sends — a whole-node export channel and
//! an authenticated SSRF in one request. Nothing here is index-scoped, so
//! there is no per-index decision to make, which is the same argument that
//! already makes snapshot and restore superuser-only.
//!
//! ## The four decisions
//!
//! **1. Where it hooks.** Nowhere on the write path. The tap reads WAL files
//! from disk on a timer; it takes no lock the writer wants and adds not one
//! instruction to `append` / `append_index_raw_batch`. That is affordable
//! because every append already drains its `BufWriter` to the page cache
//! (`WalWriter::write_one_frame_inner`), so a reader in the same process sees
//! whole frames the moment they are acked. The price is that end-to-end
//! latency has `poll_interval_ms` as its floor.
//!
//! A poll costs the bytes appended since the last one, not the size of the
//! file: `tail_shard` reads through a sliding window over a `File`
//! ([`xerj_storage::wal`]), so a caught-up shard costs one 16-byte header
//! read. It has to — the cost is multiplied by `num_wal_shards` × allowlisted
//! indices × `1000 / poll_interval_ms` per second, and `indices = ["*"]` on a
//! node with thousands of indices is a supported configuration.
//!
//! **2. Ordering and dedup.** The cursor is a **byte offset**, never a
//! sequence number: the lock-free writer reserves seq_nos outside the shard
//! mutex, so a file can hold frames out of seq order and a seq-filtered resume
//! would skip entries (see [`xerj_storage::wal::WalTailCursor`]). Delivery is
//! at-least-once — a batch that could not be confirmed is re-sent from the
//! un-advanced cursor. Every action carries `_id = doc_id` plus
//! `version_type: external` with the entry's `seq_no`, which is the same
//! "highest seq wins" rule this engine's own version map uses
//! (`VersionMap::set_if_latest`), so a redelivery or an out-of-order delivery
//! resolves to the same document the source holds. A target that ignores
//! external versioning — which currently includes xerj's own `_bulk` — falls
//! back to last-write-wins by arrival instead.
//!
//! What the target did with each action is read back per item, positionally,
//! and that is what the counters mean: `docs_shipped` and `last_shipped_seq`
//! move only for items the target **accepted**. A `_bulk` that returns `200`
//! while rejecting every action inside it — a target holding a higher external
//! version for every `_id`, i.e. a second writer got there first — is a real
//! state and used to be indistinguishable from a healthy tap.
//! `consecutive_conflict_only_batches` and
//! [`IndexTapStats::healthy`] exist so it is not.
//!
//! **3. Backpressure.** Bounded by construction: one in-flight request per
//! index, batches capped by `max_batch_docs` and `max_batch_bytes`, and
//! nothing buffered between polls. When the target is down the cursor simply
//! does not advance and the retry backs off exponentially (capped, jittered)
//! — memory does not grow, and neither does anything else on this node.
//!
//! **4a. Where a tap starts.** At the beginning of an index's WAL when
//! nothing in it has ever been pruned (generation 0 still on disk) — so an
//! index created while the tap is running is shipped whole, first document
//! included. On an index that has been running long enough to prune, at the
//! end: the surviving generations are an arbitrary recent slice, and shipping
//! them would look like a backfill without being one. **There is no backfill.**
//! Seed the target with snapshot/restore if it needs the existing documents.
//!
//! **4b. What falling behind costs.** WAL retention is *not* coupled to the
//! tap, deliberately: `prune_verified` deletes a rotated generation as soon as
//! its entries are durable in a segment, and making that wait on a remote
//! target is how a slow consumer turns into a disk-full incident. So a tap
//! that falls far enough behind loses entries — and always finds out. The
//! cursor carries the generation it sits in, a pruned-out-from-under-us
//! generation is detected exactly ([`WalShardTail::gap`]), and each occurrence
//! logs at `warn` and increments `gaps` in `GET /_xerj/wal_tap/_stats`. The
//! degradation is visible, not silent.
//!
//! "Visible" is not the same as "survivable", though, and at the default
//! `storage.flush_interval_secs = 30` the WAL window is about one flush — so a
//! minute of target downtime on a hot index loses data with nothing but a log
//! line to offer. `wal_tap.min_retained_generations` is the middle path: a
//! **bounded** floor under `prune_verified` that holds the newest N rotated
//! generations whatever any consumer is doing. Deliberately not a retention
//! lease (Elasticsearch's `ReplicationTracker.addRetentionLease`, Lucene's
//! `SnapshotDeletionPolicy.snapshot`): a lease is held until the consumer
//! releases it, which is exactly how a dead follower wedges a leader's disk.
//! A floor costs `n * wal_max_size_mb` per shard and not one byte more.
//!
//! **4c. Where the configuration lives.** In the state file next to the
//! cursors, not only in memory. `PUT /_xerj/wal_tap` is durable and is
//! re-applied over the config file on the next boot; `DELETE /_xerj/wal_tap`
//! drops it. The alternative — the shape this started as — advertised
//! "runtime config, no restart" while a restart silently reverted the tap to
//! `enabled = false`, froze its cursors, let WAL pruning carry on, and left
//! `_stats` not even listing the affected indices.
//!
//! Prior art consulted before writing this (per the repository's
//! reference-coding mandate):
//!
//! - **quickwit** `quickwit-metastore/src/checkpoint.rs:170` (`SourceCheckpoint`)
//!   and `:280-345` (`check_compatibility` / `try_apply_delta`), Apache-2.0.
//!   Its model is one position per partition, a delta that moves *backwards*
//!   is refused, and a delta that leaves a *hole* is accepted with a warning
//!   because "gaps may happen … if the indexing pipeline is down for more than
//!   the retention period". That is the same shape adopted here: one cursor
//!   per WAL shard, and a pruned generation surfaces as a counted, logged gap
//!   rather than either a stall or silence. Adapted, not copied.
//! - **elasticsearch** `x-pack/plugin/ccr/.../ShardFollowNodeTask.java:270-330`
//!   and `:586-640` — **approach only, AGPL/SSPL/Elastic-2.0, no code taken.**
//!   Confirms the two primitives: an explicitly bounded write buffer (count
//!   *and* bytes) as the whole of backpressure, and capped exponential backoff
//!   with jitter separating retryable transport failures from fatal ones.
//! - **lucene** `lucene/core/src/java/org/apache/lucene/store/BufferedIndexInput.java:289-317`
//!   (`refill`) and `:221-244` (`resolvePositionInBuffer`), plus
//!   `lucene/analysis/common/src/java/org/apache/lucene/analysis/util/SegmentingTokenizerBase.java:124-147`
//!   (`refill`, the compact-then-read step), Apache-2.0 — the shape of the
//!   incremental tail reader in [`xerj_storage::wal`]: a bounded window whose
//!   absolute position is `bufferStart + buffer.position()`, read length
//!   clamped to the file length so it never reads past EOF, and the
//!   unconsumed remainder carried to the front before pulling more. Adapted,
//!   not copied.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::{debug, info, warn};

use xerj_common::config::WalTapConfig;
use xerj_common::types::is_reserved_index;
use xerj_storage::wal::{tail_shard, wal_shard_dirs, WalEntry, WalTailCursor};

use crate::engine::Engine;

/// Filename of the durable cursor state, under the engine data directory.
const STATE_FILE: &str = "wal_tap_state.json";

/// Prefix of every index xerj creates for itself.
///
/// Broader than [`is_reserved_index`] on purpose. That predicate's constant is
/// `.xerj-memory-` (the brain namespace, which the authz layer guards); the
/// console bootstraps `.xerj_users`, `.xerj_api_tokens`, `.xerj_sessions`,
/// `.xerj_magic_links` and friends, which hold hashed credentials and live
/// sessions. Nothing under either shape may leave this node.
const XERJ_SYSTEM_INDEX_PREFIX: &str = ".xerj";

/// Is `index` one of xerj's own?
fn is_xerj_system_index(index: &str) -> bool {
    index.starts_with(XERJ_SYSTEM_INDEX_PREFIX) || is_reserved_index(index)
}

/// Persisted format version. Bump on any incompatible change to
/// [`PersistedState`]; an unrecognised version is refused rather than
/// reinterpreted, because misreading a cursor means silently re-shipping or
/// silently skipping.
const STATE_VERSION: u32 = 1;

// ─────────────────────────────────────────────────────────────────────────────
// Durable cursor state
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct IndexCursors {
    /// WAL shard name (`"s0"`, or `""` for the legacy single-WAL layout) →
    /// position.
    shards: BTreeMap<String, WalTailCursor>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedState {
    version: u32,
    indices: BTreeMap<String, IndexCursors>,
    /// The last configuration applied through `PUT /_xerj/wal_tap`.
    ///
    /// Without this, "runtime config, no restart" was a half-truth that
    /// failed in the direction that loses data: the live config lived only in
    /// an `RwLock`, so a restart silently reverted the tap to the file config
    /// — typically `enabled = false`. Cursors then froze while WAL pruning
    /// carried on, and `_stats` did not even list the affected indices,
    /// because it only lists what the *current* allowlist matches.
    ///
    /// Precedent for a persisted overlay winning over the file is
    /// Elasticsearch's `persistent` cluster settings.
    /// `DELETE /_xerj/wal_tap` drops it and reverts to the file.
    #[serde(default)]
    runtime_config: Option<WalTapConfig>,
}

impl Default for PersistedState {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            indices: BTreeMap::new(),
            runtime_config: None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Stats
// ─────────────────────────────────────────────────────────────────────────────

/// Per-index counters, all monotonic within a process lifetime.
#[derive(Debug, Clone, Default, Serialize)]
pub struct IndexTapStats {
    /// `index` actions accepted into a `_bulk` body.
    pub docs_shipped: u64,
    /// `delete` actions accepted into a `_bulk` body.
    pub deletes_shipped: u64,
    /// `_bulk` requests the target answered 2xx to.
    pub batches_sent: u64,
    /// Requests that failed outright (transport error or non-2xx). The cursor
    /// does not advance for these — they are retried.
    pub bulk_failures: u64,
    /// Items the target rejected inside an otherwise-successful `_bulk`
    /// response. The cursor DOES advance past these; see [`WalTap::send_bulk`].
    pub item_failures: u64,
    /// Version conflicts inside a `_bulk` response. Expected and benign: they
    /// are how external versioning absorbs a redelivery. Counted separately so
    /// they never look like data loss.
    pub version_conflicts: u64,
    /// Times a poll found a WAL generation pruned before it could be shipped.
    /// Any value above zero means entries the target will never receive.
    ///
    /// Counted per observation, not per lost generation: while a tap is stuck
    /// behind a live gap, each poll re-observes it and this keeps climbing.
    /// That is the useful reading — "still losing data", not "lost data once".
    pub gaps: u64,
    /// `UpdateMapping` WAL entries seen. No current xerj code path writes one
    /// (only read sites exist), so this is expected to stay 0; if it ever
    /// moves, the target's dynamic mapping is what handled the change.
    pub mapping_updates_skipped: u64,
    /// Frames skipped because their payload did not decode.
    pub corrupt_skipped: u64,
    /// Consecutive failed sends; drives the backoff.
    pub consecutive_failures: u32,
    /// Last failure, as a short string. `None` once a send succeeds.
    pub last_error: Option<String>,
    /// Highest `seq_no` the target **accepted**, for the lag estimate.
    ///
    /// Accepted means the item came back without an `error`. A rejected item
    /// — including a version conflict — does not move this, because the whole
    /// point of the number is to answer "is our data there?" and the answer
    /// for a rejected action is no. Deriving it from the render-time batch
    /// instead is what let a target that rejected every single action report
    /// `lag_seq: 0` while `docs_shipped` climbed.
    pub last_shipped_seq: u64,
    /// Consecutive `_bulk` responses in which the target rejected **every**
    /// action as a version conflict.
    ///
    /// One conflict is expected and benign: it is how external versioning
    /// absorbs an at-least-once redelivery. A *sustained run* of batches that
    /// are entirely conflicts is not — it means the target holds a higher
    /// version for every `_id` this tap sends, so the tap is a no-op and the
    /// two sides are diverging. See [`IndexTapStats::healthy`].
    pub consecutive_conflict_only_batches: u32,
}

/// How many all-conflict batches in a row stop counting as "a redelivery
/// landed" and start counting as "this target is not taking our writes".
///
/// A redelivery after a failed send re-ships exactly the chunks that did not
/// land, so it clears on the next batch that contains anything new. Three in a
/// row cannot be that.
const CONFLICT_ONLY_UNHEALTHY_AFTER: u32 = 3;

impl IndexTapStats {
    /// Is this index's replication actually working?
    ///
    /// `false` for the two states that used to look identical to a healthy
    /// tap in `_stats`: a send that keeps failing, and a target that accepts
    /// every request while rejecting every action inside it.
    pub fn healthy(&self) -> bool {
        self.last_error.is_none()
            && self.consecutive_conflict_only_batches < CONFLICT_ONLY_UNHEALTHY_AFTER
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// The tap
// ─────────────────────────────────────────────────────────────────────────────

/// Shared, runtime-mutable tap state. Held by the [`Engine`] so the REST
/// surface can read stats and edit the allowlist without a restart.
pub struct WalTap {
    config: RwLock<WalTapConfig>,
    state: RwLock<PersistedState>,
    stats: RwLock<BTreeMap<String, IndexTapStats>>,
    state_path: PathBuf,
    client: reqwest::Client,
    /// Monotonic tick counter, so tests can observe that the loop ran.
    ticks: AtomicU64,
    /// Milliseconds-since-start before which the next send must not be tried.
    retry_not_before_ms: AtomicU64,
    /// Cursor state has moved since the last durable write.
    ///
    /// Cursors are flushed **once per tick**, not once per index per poll.
    /// The old shape re-serialised and fsynced the WHOLE state file on every
    /// cursor advance — O(N²) bytes and 2N fsyncs per tick at N allowlisted
    /// indices, at a 500 ms default, on a node that `index_store.rs:806-813`
    /// records reaching 9,382 indices. `indices = ["*"]` walked straight into
    /// that.
    ///
    /// Deferring costs nothing that matters: the cursor is only ever
    /// persisted *after* the entries it covers have been shipped, so a crash
    /// in the window re-delivers a batch. Re-delivery is the documented
    /// contract (at-least-once, absorbed by `version_type: external`); losing
    /// a batch would not be, and that direction is impossible here.
    dirty: std::sync::atomic::AtomicBool,
    /// Durable writes of the state file since start.
    ///
    /// Instrumentation, and the only way to assert the property that matters
    /// about [`WalTap::dirty`]: the cost of a tick must not scale with the
    /// number of allowlisted indices. See
    /// `cursor_state_is_written_once_per_tick_not_once_per_index`.
    persists: AtomicU64,
    started: std::time::Instant,
}

impl WalTap {
    /// Build a tap and load any persisted cursors from `data_dir`.
    ///
    /// A state file that cannot be parsed is refused loudly and treated as
    /// absent — which means every allowlisted index restarts "from now"
    /// instead of re-shipping from a position that may be nonsense.
    pub fn new(data_dir: &Path, config: WalTapConfig) -> Self {
        let state_path = data_dir.join(STATE_FILE);
        let state = match std::fs::read(&state_path) {
            Ok(bytes) => match serde_json::from_slice::<PersistedState>(&bytes) {
                Ok(s) if s.version == STATE_VERSION => s,
                Ok(s) => {
                    warn!(
                        found = s.version,
                        expected = STATE_VERSION,
                        "WAL tap state file has an unknown version — starting from now"
                    );
                    PersistedState::default()
                }
                Err(e) => {
                    warn!(error = %e, path = ?state_path,
                          "WAL tap state file is unreadable — starting from now");
                    PersistedState::default()
                }
            },
            Err(_) => PersistedState::default(),
        };

        // A configuration set through the API outlives the process that took
        // it. Without this a restart reverted to the file config — usually
        // `enabled = false` — and the tap went quiet while its cursors froze
        // and WAL pruning carried on regardless.
        let config = match state.runtime_config.clone() {
            Some(saved) => {
                info!(
                    target_url = %saved.redacted_target_url(),
                    enabled = saved.enabled,
                    indices = ?saved.indices,
                    "WAL tap: applying the configuration last set via PUT /_xerj/wal_tap \
                     (overrides the config file; DELETE /_xerj/wal_tap reverts)"
                );
                saved
            }
            None => config,
        };

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.request_timeout_secs.max(1)))
            .build()
            .unwrap_or_default();

        Self {
            config: RwLock::new(config),
            state: RwLock::new(state),
            stats: RwLock::new(BTreeMap::new()),
            state_path,
            client,
            ticks: AtomicU64::new(0),
            retry_not_before_ms: AtomicU64::new(0),
            dirty: std::sync::atomic::AtomicBool::new(false),
            persists: AtomicU64::new(0),
            started: std::time::Instant::now(),
        }
    }

    /// Snapshot of the live configuration.
    pub fn config(&self) -> WalTapConfig {
        self.config.read().clone()
    }

    /// Replace the live configuration **durably**.
    ///
    /// Takes effect on the next poll — no restart — and cursors are preserved
    /// so an index that stays in the allowlist does not re-ship or skip. The
    /// configuration is also written next to the cursors and re-applied over
    /// the file config on the next boot, so "no restart needed" does not
    /// quietly mean "and it is gone after one".
    pub fn set_config(&self, config: WalTapConfig) {
        *self.config.write() = config.clone();
        self.state.write().runtime_config = Some(config);
        // Operator action, not a hot path: make it durable now rather than at
        // the next tick, so a crash between the 200 and the next poll cannot
        // lose an acknowledged configuration change.
        self.persist_now();
    }

    /// Drop the persisted runtime overlay and revert to `config` (the value
    /// from the config file).
    pub fn clear_runtime_config(&self, file_config: WalTapConfig) {
        *self.config.write() = file_config;
        self.state.write().runtime_config = None;
        self.persist_now();
    }

    /// Does a configuration set through the API currently override the file?
    pub fn has_runtime_config(&self) -> bool {
        self.state.read().runtime_config.is_some()
    }

    /// Number of completed poll cycles — the "it is actually running" signal
    /// for tests and for `_stats`.
    pub fn ticks(&self) -> u64 {
        self.ticks.load(Ordering::Relaxed)
    }

    /// Per-index counters.
    pub fn stats(&self) -> BTreeMap<String, IndexTapStats> {
        self.stats.read().clone()
    }

    /// Put an index's cursors back, verbatim.
    ///
    /// Test seam. Reconstructs the one state no in-process sequence can reach
    /// on its own: a persisted cursor that outlived its WAL stream because the
    /// delete happened while this process was **not running**, so
    /// [`forget_index`](Self::forget_index) never fired. That is the case
    /// `tail_shard`'s offset-past-EOF check exists for, and it is unreachable
    /// from the public API by construction.
    #[doc(hidden)]
    pub fn restore_cursors_for_test(&self, index: &str, shards: BTreeMap<String, WalTailCursor>) {
        self.state
            .write()
            .indices
            .entry(index.to_string())
            .or_default()
            .shards = shards;
        self.persist_now();
    }

    /// Rewind an index's cursors to the start of their oldest generation.
    ///
    /// Test seam for the redelivery half of the at-least-once contract: it
    /// reproduces exactly what a crash between shipping a batch and persisting
    /// its cursor leaves behind, which is the case `version_type: external`
    /// exists to make safe.
    #[doc(hidden)]
    pub fn rewind_for_test(&self, index: &str) {
        let mut state = self.state.write();
        if let Some(cursors) = state.indices.get_mut(index) {
            for cursor in cursors.shards.values_mut() {
                *cursor = WalTailCursor::start_of(0);
            }
        }
        drop(state);
        self.persist_now();
    }

    /// Per-index, per-shard cursors, for `_stats`.
    pub fn cursors(&self) -> BTreeMap<String, BTreeMap<String, WalTailCursor>> {
        self.state
            .read()
            .indices
            .iter()
            .map(|(k, v)| (k.clone(), v.shards.clone()))
            .collect()
    }

    /// Does `index` match the allowlist, and is it allowed to be shipped at
    /// all?
    ///
    /// Two rules sit above the patterns:
    ///
    /// 1. **xerj's own system indices are never shipped**, whatever the
    ///    allowlist says. That means every `.xerj*` name, not just the
    ///    [`is_reserved_index`] namespace: that constant is `.xerj-memory-`,
    ///    which covers the brain indices but not `.xerj_users`,
    ///    `.xerj_api_tokens` or `.xerj_sessions` — where hashed credentials,
    ///    tokens and console sessions live. An `indices = ["*"]` tap that
    ///    shipped those to another cluster would be a credential leak, and it
    ///    is what the regression test
    ///    `system_indices_are_never_pushed_even_with_a_star_allowlist` exists
    ///    to catch.
    /// 2. **A wildcard does not expand to a hidden index.** `["*"]` means
    ///    "every ordinary index", not "also `.kibana`" — the same rule
    ///    Elasticsearch's `expand_wildcards` uses, where a pattern reaches a
    ///    dotted index only when the pattern is itself dotted. Naming one
    ///    outright (`indices = [".kibana"]`) still works.
    pub fn ships(config: &WalTapConfig, index: &str) -> bool {
        if is_xerj_system_index(index) {
            return false;
        }
        config.indices.iter().any(|pat| {
            if index.starts_with('.') && !pat.starts_with('.') {
                return false;
            }
            crate::engine::glob_match(pat, index)
        })
    }

    fn stat<R>(&self, index: &str, f: impl FnOnce(&mut IndexTapStats) -> R) -> R {
        let mut guard = self.stats.write();
        f(guard.entry(index.to_string()).or_default())
    }

    /// Note that cursor state has moved. The write happens once, at the end of
    /// the tick — see [`WalTap::dirty`] for why deferring is safe.
    fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Flush cursor state if anything moved since the last write.
    fn persist_if_dirty(&self) {
        if self.dirty.swap(false, Ordering::Relaxed) {
            self.persist_now();
        }
    }

    /// Durable writes of the state file since start.
    ///
    /// The tap's I/O amplification, exposed so it can be asserted rather than
    /// asserted about: one tick must cost one write however many indices are
    /// allowlisted.
    pub fn persists(&self) -> u64 {
        self.persists.load(Ordering::Relaxed)
    }

    fn persist_now(&self) {
        self.dirty.store(false, Ordering::Relaxed);
        self.persists.fetch_add(1, Ordering::Relaxed);
        let snapshot = self.state.read().clone();
        let bytes = match serde_json::to_vec_pretty(&snapshot) {
            Ok(b) => b,
            Err(e) => {
                warn!(error = %e, "WAL tap: cannot serialise cursor state");
                return;
            }
        };
        // Durable replace at 0600: a torn cursor file is a silent re-ship or a
        // silent skip on the next boot, and the persisted runtime config in
        // this file carries `target_auth` — a credential for another cluster,
        // which must not be world-readable in the data directory. Same writer
        // the API-key store uses.
        if let Err(e) = crate::engine::write_secret_file_atomic(&self.state_path, &bytes) {
            warn!(error = %e, path = ?self.state_path, "WAL tap: cannot persist cursor state");
        }
    }

    fn now_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    fn in_backoff(&self) -> bool {
        self.now_ms() < self.retry_not_before_ms.load(Ordering::Relaxed)
    }

    /// Capped exponential backoff with jitter, keyed off the worst
    /// consecutive-failure count across indices.
    ///
    /// Approach (not code) from Elasticsearch's `ShardFollowNodeTask.computeDelay`.
    fn arm_backoff(&self, config: &WalTapConfig, failures: u32) {
        let base = config.poll_interval_ms.max(50);
        let shift = failures.saturating_sub(1).min(20);
        let uncapped = base.saturating_mul(1u64 << shift);
        let cap = config.max_retry_backoff_secs.max(1) * 1000;
        let delay = uncapped.min(cap);
        // Deterministic, dependency-free jitter in [50%, 100%] of the delay:
        // enough to stop N indices from re-hammering a recovering target in
        // lockstep, without pulling in an RNG.
        let jitter = self.now_ms() % 50 + 50;
        let delay = delay * jitter / 100;
        self.retry_not_before_ms
            .store(self.now_ms() + delay, Ordering::Relaxed);
    }

    fn clear_backoff(&self) {
        self.retry_not_before_ms.store(0, Ordering::Relaxed);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Bulk payload
// ─────────────────────────────────────────────────────────────────────────────

/// One WAL entry rendered as its two (or one) `_bulk` NDJSON lines.
///
/// `version_type: external` with the WAL `seq_no` as the version is what makes
/// at-least-once safe: the target keeps the highest-seq version of each
/// `_id`, which is the same winner [`xerj_storage::version_map::VersionMap`]
/// picks locally.
fn render_action(index: &str, seq_no: u64, entry: &WalEntry, out: &mut String) -> Option<bool> {
    match entry {
        WalEntry::Index { doc_id, source } => {
            let meta = json!({"index": {
                "_index": index,
                "_id": doc_id,
                "version": seq_no,
                "version_type": "external",
            }});
            out.push_str(&meta.to_string());
            out.push('\n');
            out.push_str(&compact_source(source));
            out.push('\n');
            Some(false)
        }
        WalEntry::Delete { doc_id } => {
            let meta = json!({"delete": {
                "_index": index,
                "_id": doc_id,
                "version": seq_no,
                "version_type": "external",
            }});
            out.push_str(&meta.to_string());
            out.push('\n');
            Some(true)
        }
        // Never written by any current code path (grep: only read sites
        // exist). If one ever appears, the target's dynamic mapping is what
        // absorbs the change — forwarding xerj's internal schema would mean
        // translating it into an ES mapping, a second translation surface with
        // its own failure modes, for an entry that does not occur.
        WalEntry::UpdateMapping { .. } => None,
    }
}

/// A document source as one NDJSON line. A WAL source is always a JSON value
/// with no interior newlines once re-serialised compactly, which is what
/// `_bulk` requires.
fn compact_source(source: &Value) -> String {
    source.to_string()
}

/// One rendered action, in the order it appears in the `_bulk` body.
///
/// The `items` array of a `_bulk` response is positionally aligned with the
/// actions of the request, which is the only way to say *which* seq_no the
/// target actually took. Keeping the pairing is what makes
/// [`IndexTapStats::last_shipped_seq`] mean something.
#[derive(Clone, Copy)]
struct RenderedAction {
    seq: u64,
    is_delete: bool,
}

/// One `_bulk` request's worth of rendered actions.
struct Chunk {
    body: String,
    actions: Vec<RenderedAction>,
}

/// Split a batch into `_bulk` bodies no larger than `max_bytes`.
///
/// A single action larger than `max_bytes` gets a chunk of its own rather than
/// being dropped: the cap is there to respect the target's
/// `http.max_content_length`, and silently discarding an oversized document
/// would be data loss dressed up as a limit.
fn render_chunks(index: &str, batch: &[(u64, WalEntry)], max_bytes: usize) -> Vec<Chunk> {
    let max_bytes = max_bytes.max(1);
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut cur = Chunk {
        body: String::new(),
        actions: Vec::new(),
    };

    for (seq, entry) in batch {
        let mut rendered = String::new();
        let Some(is_delete) = render_action(index, *seq, entry, &mut rendered) else {
            continue;
        };
        if !cur.body.is_empty() && cur.body.len() + rendered.len() > max_bytes {
            chunks.push(std::mem::replace(
                &mut cur,
                Chunk {
                    body: String::new(),
                    actions: Vec::new(),
                },
            ));
        }
        cur.body.push_str(&rendered);
        cur.actions.push(RenderedAction {
            seq: *seq,
            is_delete,
        });
    }
    if !cur.body.is_empty() {
        chunks.push(cur);
    }
    chunks
}

// ─────────────────────────────────────────────────────────────────────────────
// The poll loop
// ─────────────────────────────────────────────────────────────────────────────

/// What the target did with one action inside a 2xx `_bulk` response.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ItemOutcome {
    /// The target took it. This — and only this — advances the watermark.
    Accepted,
    /// `version_conflict_engine_exception`: the target already holds a
    /// version at least as high for this `_id`. Benign once (that is how
    /// external versioning absorbs a redelivery), a divergence signal when it
    /// is *every* action of *every* batch.
    Conflict,
    /// Rejected for any other reason — a mapping conflict, say. Counted,
    /// logged, and passed over: see [`WalTap::send_bulk`].
    Rejected,
}

/// Outcome of shipping one `_bulk` request.
enum SendOutcome {
    /// The target accepted the request. Per-item failures may still have
    /// occurred; the cursor advances either way.
    ///
    /// `outcomes` is positionally aligned with the chunk's actions.
    Accepted { outcomes: Vec<ItemOutcome> },
    /// The request did not land. The cursor must not advance.
    Failed(String),
}

impl WalTap {
    /// POST one NDJSON body to `{target}/_bulk`.
    ///
    /// Whole-request failures (transport error, any non-2xx) leave the cursor
    /// where it is, so nothing is dropped and the batch is retried. Per-item
    /// failures inside a 2xx response are counted, logged, and *passed over*:
    /// a document the target will reject deterministically — a mapping
    /// conflict, say — would otherwise wedge the tap forever, and a wedged tap
    /// loses everything behind it to WAL pruning rather than just the one bad
    /// document. That trade is deliberate and is why `item_failures` is in
    /// `_stats`.
    async fn send_bulk(&self, config: &WalTapConfig, body: String, actions: usize) -> SendOutcome {
        let url = format!("{}/_bulk", config.target_url.trim_end_matches('/'));
        let mut req = self
            .client
            .post(&url)
            .header("Content-Type", "application/x-ndjson")
            .timeout(Duration::from_secs(config.request_timeout_secs.max(1)))
            .body(body);
        if !config.target_auth.is_empty() {
            req = req.header("Authorization", config.target_auth.clone());
        }

        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => return SendOutcome::Failed(format!("transport: {e}")),
        };
        let status = resp.status();
        if !status.is_success() {
            let snippet = resp
                .text()
                .await
                .unwrap_or_default()
                .chars()
                .take(300)
                .collect::<String>();
            return SendOutcome::Failed(format!("HTTP {status}: {snippet}"));
        }

        let parsed: Value = match resp.json().await {
            Ok(v) => v,
            // A 2xx the target could not describe. Treat as landed — resending
            // would double-deliver for no new information.
            Err(e) => {
                debug!(error = %e, "WAL tap: unparseable _bulk response body");
                return SendOutcome::Accepted {
                    outcomes: vec![ItemOutcome::Accepted; actions],
                };
            }
        };

        // `errors: false` means every action landed, and ES omits nothing in
        // that case worth walking.
        if parsed.get("errors").and_then(Value::as_bool) != Some(true) {
            return SendOutcome::Accepted {
                outcomes: vec![ItemOutcome::Accepted; actions],
            };
        }

        let items = parsed
            .get("items")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        if items.len() != actions {
            // `errors: true` with an items array that is not positionally
            // aligned with the request. Nothing can be attributed to a
            // specific action, so refuse to claim any of them landed: hold
            // the cursor and retry rather than advance past writes whose fate
            // is unknown. At-least-once is a contract; at-most-once is not.
            return SendOutcome::Failed(format!(
                "target reported errors with {} items for {actions} actions — cannot \
                 attribute outcomes, holding the cursor",
                items.len()
            ));
        }

        let mut outcomes = Vec::with_capacity(items.len());
        let mut warned = false;
        for item in items {
            let action = item.as_object().and_then(|o| o.values().next());
            let error = action.and_then(|a| a.get("error"));
            let Some(error) = error else {
                outcomes.push(ItemOutcome::Accepted);
                continue;
            };
            let kind = error.get("type").and_then(Value::as_str).unwrap_or("");
            if kind == "version_conflict_engine_exception" {
                outcomes.push(ItemOutcome::Conflict);
            } else {
                if !warned {
                    warned = true;
                    warn!(error = %error, "WAL tap: target rejected a document — \
                          skipped (see item_failures in _stats)");
                }
                outcomes.push(ItemOutcome::Rejected);
            }
        }
        SendOutcome::Accepted { outcomes }
    }

    /// Poll one index once: read its WAL tail, ship it, advance its cursors.
    ///
    /// `head_seq` is the index's highest reserved sequence number, used only to
    /// seed the lag watermark on a first look — see below.
    ///
    /// Returns `true` when there is more to read immediately (the batch was
    /// truncated), so the caller can keep going without waiting a full poll
    /// interval.
    async fn poll_index(
        &self,
        config: &WalTapConfig,
        name: &str,
        wal_root: PathBuf,
        head_seq: u64,
    ) -> bool {
        let known: BTreeMap<String, WalTailCursor> = self
            .state
            .read()
            .indices
            .get(name)
            .map(|c| c.shards.clone())
            .unwrap_or_default();
        let first_look = known.is_empty();

        let budget = config.max_batch_docs.max(1);
        let read = {
            let known = known.clone();
            // WAL reads are blocking file I/O; keep them off the async
            // runtime's worker threads.
            match tokio::task::spawn_blocking(move || {
                let mut shards = Vec::new();
                for (shard, dir) in wal_shard_dirs(&wal_root) {
                    let cursor = known.get(&shard).copied();
                    let tail = tail_shard(&dir, cursor, budget);
                    shards.push((shard, tail));
                }
                shards
            })
            .await
            {
                Ok(v) => v,
                Err(e) => {
                    warn!(index = %name, error = %e, "WAL tap: WAL read task failed");
                    return false;
                }
            }
        };

        // Merge the shard streams. Every entry read this poll is shipped:
        // the per-shard cursors advance together with the one request, so a
        // partial-batch horizon is unnecessary — each shard either ships all
        // of what it read or none of it.
        let mut batch: Vec<(u64, WalEntry)> = Vec::new();
        let mut new_cursors: BTreeMap<String, WalTailCursor> = known.clone();
        let mut more = false;
        let mut gaps = 0u64;
        let mut corrupt = 0u64;
        let mut mapping_updates = 0u64;

        for (shard, tail) in read {
            if tail.gap {
                gaps += 1;
                warn!(
                    index = %name, shard = %shard,
                    "WAL tap: a discontinuity in this shard's WAL stream — either a \
                     generation was pruned before it could be shipped, or the stream was \
                     replaced (index deleted and recreated, or data directory rolled \
                     back). Entries are missing at the target and cannot be recovered \
                     from the WAL. If the tap is behind, raise poll throughput, check the \
                     target, or set wal_tap.min_retained_generations; `tail_shard` names \
                     which of the two it was at WARN."
                );
            }
            corrupt += tail.corrupt_skipped as u64;
            more |= tail.truncated;
            for entry in tail.entries {
                if matches!(entry.entry, WalEntry::UpdateMapping { .. }) {
                    mapping_updates += 1;
                }
                batch.push((entry.seq_no, entry.entry));
            }
            new_cursors.insert(shard, tail.cursor);
        }

        if gaps > 0 || corrupt > 0 || mapping_updates > 0 {
            self.stat(name, |s| {
                s.gaps += gaps;
                s.corrupt_skipped += corrupt;
                s.mapping_updates_skipped += mapping_updates;
            });
        }

        if batch.is_empty() {
            // Nothing to send, but the cursors may still have moved (a fresh
            // start-at-the-end position, or frames that decoded to nothing).
            if new_cursors != known {
                self.state
                    .write()
                    .indices
                    .entry(name.to_string())
                    .or_default()
                    .shards = new_cursors;
                self.mark_dirty();
                if first_look {
                    // The tap just positioned at the end of an established
                    // index: it is caught up by definition, having deliberately
                    // skipped the history it cannot backfill. Seeding the
                    // watermark keeps `lag_seq` from reporting the whole of
                    // that history as a backlog the tap intends to ship.
                    self.stat(name, |s| {
                        s.last_shipped_seq = s.last_shipped_seq.max(head_seq);
                    });
                }
            }
            return more;
        }

        // Ship in seq order. Byte order is what the cursor resumes from, but
        // seq order is what decides which write wins — both here inside one
        // batch, and at any target that honours external versioning.
        batch.sort_by_key(|(seq, _)| *seq);

        let chunks = render_chunks(name, &batch, config.max_batch_bytes);
        if chunks.is_empty() {
            // Everything read was an entry kind that renders to nothing.
            self.state
                .write()
                .indices
                .entry(name.to_string())
                .or_default()
                .shards = new_cursors;
            self.mark_dirty();
            return more;
        }

        // `max_batch_bytes` bounds one HTTP request, not one poll: a target's
        // `http.max_content_length` is what it exists for. Every chunk of the
        // read has to land before the cursors move, so a failure part-way
        // through redelivers the earlier chunks on the next poll — which is
        // exactly what external versioning makes free.
        // Everything here is counted from the target's PER-ITEM verdict, not
        // from what was rendered. A `_bulk` that returns 200 while rejecting
        // every action inside it is a common, real state — a target holding
        // higher external versions for the same `_id`s makes every action a
        // no-op — and counting render-time values made that indistinguishable
        // from a healthy tap: `docs_shipped` climbed, `lag_seq` read 0, and
        // nothing had been replicated.
        let mut docs = 0u64;
        let mut deletes = 0u64;
        let mut max_accepted_seq = 0u64;
        let mut item_failures = 0u64;
        let mut version_conflicts = 0u64;
        let mut batches = 0u64;
        let mut conflict_only_batches = 0u32;

        for chunk in chunks {
            let actions = chunk.actions.clone();
            match self.send_bulk(config, chunk.body, actions.len()).await {
                SendOutcome::Accepted { outcomes } => {
                    let mut accepted_here = 0u64;
                    let mut conflicts_here = 0u64;
                    for (action, outcome) in actions.iter().zip(outcomes.iter()) {
                        match outcome {
                            ItemOutcome::Accepted => {
                                accepted_here += 1;
                                max_accepted_seq = max_accepted_seq.max(action.seq);
                                if action.is_delete {
                                    deletes += 1;
                                } else {
                                    docs += 1;
                                }
                            }
                            ItemOutcome::Conflict => {
                                conflicts_here += 1;
                                version_conflicts += 1;
                            }
                            ItemOutcome::Rejected => item_failures += 1,
                        }
                    }
                    if accepted_here == 0 && conflicts_here == actions.len() as u64 {
                        conflict_only_batches += 1;
                    }
                    batches += 1;
                }
                SendOutcome::Failed(err) => {
                    let failures = self.stat(name, |s| {
                        s.docs_shipped += docs;
                        s.deletes_shipped += deletes;
                        s.batches_sent += batches;
                        s.item_failures += item_failures;
                        s.version_conflicts += version_conflicts;
                        s.last_shipped_seq = s.last_shipped_seq.max(max_accepted_seq);
                        s.bulk_failures += 1;
                        s.consecutive_failures = s.consecutive_failures.saturating_add(1);
                        s.last_error = Some(err.clone());
                        s.consecutive_failures
                    });
                    warn!(index = %name, error = %err, failures,
                          "WAL tap: _bulk failed — cursor held, will retry");
                    self.arm_backoff(config, failures);
                    return false;
                }
            }
        }

        self.state
            .write()
            .indices
            .entry(name.to_string())
            .or_default()
            .shards = new_cursors;
        self.mark_dirty();
        let unhealthy = self.stat(name, |s| {
            s.docs_shipped += docs;
            s.deletes_shipped += deletes;
            s.batches_sent += batches;
            s.item_failures += item_failures;
            s.version_conflicts += version_conflicts;
            s.last_shipped_seq = s.last_shipped_seq.max(max_accepted_seq);
            s.consecutive_failures = 0;
            // A run of batches in which the target rejected every action as a
            // conflict is not "replication working". Only a batch that landed
            // something clears it.
            if conflict_only_batches > 0 && docs == 0 && deletes == 0 {
                s.consecutive_conflict_only_batches = s
                    .consecutive_conflict_only_batches
                    .saturating_add(conflict_only_batches);
            } else {
                s.consecutive_conflict_only_batches = 0;
            }
            if s.consecutive_conflict_only_batches >= CONFLICT_ONLY_UNHEALTHY_AFTER {
                s.last_error = Some(format!(
                    "target rejected every action of the last {} batches with \
                     version_conflict_engine_exception — it holds a higher version for \
                     every document this tap sends, so nothing is being replicated",
                    s.consecutive_conflict_only_batches
                ));
                true
            } else {
                s.last_error = None;
                false
            }
        });
        if unhealthy {
            warn!(
                index = %name, version_conflicts,
                "WAL tap: every action is being rejected as a version conflict — the \
                 target holds higher versions than this node. _stats reports the index \
                 unhealthy; check that the target is not also fed by another writer."
            );
        }
        self.clear_backoff();
        debug!(index = %name, docs, deletes, batches, "WAL tap: batch shipped");
        more
    }

    /// Drop one index's cursors the moment it is deleted.
    ///
    /// Called from [`Engine::delete_index`](crate::engine::Engine::delete_index),
    /// and that timing is the whole point. A deletion is the exact boundary
    /// between two incarnations of a name: after it, any surviving cursor
    /// belongs to a WAL stream that no longer exists.
    ///
    /// Waiting for a poll to *notice* the absence — which is all
    /// [`forget_deleted`](Self::forget_deleted) can do — misses the case that
    /// matters, because `DELETE` + `PUT` inside one 500 ms poll interval is
    /// never observed as an absence at all. The recreated index then starts a
    /// new WAL at generation 0, so the `cursor.generation > newest` guard
    /// never trips, and the stale byte offset either landed past EOF (every
    /// document silently skipped, `gap` false) or mid-frame (the shard wedged
    /// forever with `gaps: 0` and `last_error: null`). Both are the failure a
    /// WAL consumer must never have, and both are unreachable from here.
    ///
    /// `tail_shard`'s offset-past-EOF check is the second line of defence,
    /// for a delete this process never saw — one that happened while the node
    /// was down.
    pub fn forget_index(&self, name: &str) {
        let removed = self.state.write().indices.remove(name).is_some();
        self.stats.write().remove(name);
        if removed {
            debug!(index = %name, "WAL tap: forgetting the cursor of a deleted index");
            self.persist_now();
        }
    }

    /// Drop cursors for indices that no longer exist.
    ///
    /// Hygiene for anything [`forget_index`](Self::forget_index) did not see:
    /// a deletion by a path that does not route through `Engine::delete_index`,
    /// or an index removed while this process was not running.
    fn forget_deleted(&self, live: &[String]) {
        let mut state = self.state.write();
        if state.indices.keys().all(|k| live.iter().any(|l| l == k)) {
            return;
        }
        state.indices.retain(|name, _| {
            let kept = live.iter().any(|l| l == name);
            if !kept {
                debug!(index = %name, "WAL tap: forgetting the cursor of a deleted index");
            }
            kept
        });
        drop(state);
        self.mark_dirty();
    }

    /// One full pass over the allowlisted indices. Exposed for tests; the
    /// server drives [`run`] instead.
    pub async fn tick(&self, engine: &Engine) {
        let config = self.config();
        self.ticks.fetch_add(1, Ordering::Relaxed);
        if !config.enabled || config.target_url.trim().is_empty() || config.indices.is_empty() {
            return;
        }
        if self.in_backoff() {
            return;
        }

        let live = engine.index_name_list();
        self.forget_deleted(&live);
        // One durable write per tick, at the end, whatever moved — not one
        // per index per poll. See `WalTap::dirty`.
        let _flush = FlushOnDrop(self);

        for name in live {
            if !Self::ships(&config, &name) {
                continue;
            }
            let Ok(index) = engine.get_index(&name) else {
                continue;
            };
            let wal_root = index.wal_dir();
            // `current_seq_no` is the NEXT seq to be handed out, so the highest
            // reserved one is that minus 1.
            let head_seq = index.current_seq_no().saturating_sub(1);
            // Keep draining an index that is behind, but bounded: at most a
            // few batches per tick so one busy index cannot starve the others.
            for _ in 0..4 {
                if !self
                    .poll_index(&config, &name, wal_root.clone(), head_seq)
                    .await
                {
                    break;
                }
            }
            if self.in_backoff() {
                break;
            }
        }
    }
}

/// Flushes the tap's cursor state when a tick ends, however it ends.
///
/// A guard rather than a call at the bottom of `tick`, because `tick` returns
/// early from several places (backoff armed mid-loop, a send failure) and a
/// missed flush means the *next* boot re-ships from an older cursor.
struct FlushOnDrop<'a>(&'a WalTap);

impl Drop for FlushOnDrop<'_> {
    fn drop(&mut self) {
        self.0.persist_if_dirty();
    }
}

/// Drive the tap until the process exits.
pub async fn run(tap: Arc<WalTap>, engine: Arc<Engine>) {
    info!("WAL tap loop started (disabled until wal_tap.enabled and a target are set)");
    loop {
        let interval = Duration::from_millis(tap.config().poll_interval_ms.clamp(50, 60_000));
        tokio::time::sleep(interval).await;
        tap.tick(&engine).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(indices: &[&str]) -> WalTapConfig {
        WalTapConfig {
            enabled: true,
            target_url: "http://localhost:1".into(),
            indices: indices.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn allowlist_matches_globs() {
        let c = cfg(&["edge-*", "metrics"]);
        assert!(WalTap::ships(&c, "edge-logs"));
        assert!(WalTap::ships(&c, "metrics"));
        assert!(!WalTap::ships(&c, "orders"));
    }

    #[test]
    fn system_indices_are_never_shipped_even_by_a_star_allowlist() {
        let c = cfg(&["*"]);
        assert!(WalTap::ships(&c, "edge-logs"));
        assert!(!WalTap::ships(&c, ".xerj-memory-alice"));
        assert!(!WalTap::ships(&c, ".xerj_users"));
        assert!(!WalTap::ships(&c, ".xerj_api_tokens"));
        // …and naming one outright does not get round it either.
        assert!(!WalTap::ships(&cfg(&[".xerj_users"]), ".xerj_users"));
        assert!(!WalTap::ships(&cfg(&[".xerj*"]), ".xerj_sessions"));
    }

    /// A wildcard means "every ordinary index", not "also the hidden ones" —
    /// the same rule ES's `expand_wildcards` uses. An explicitly dotted
    /// pattern still reaches them.
    #[test]
    fn a_wildcard_does_not_expand_to_hidden_indices() {
        assert!(!WalTap::ships(&cfg(&["*"]), ".kibana"));
        assert!(WalTap::ships(&cfg(&[".kibana"]), ".kibana"));
        assert!(WalTap::ships(&cfg(&[".kib*"]), ".kibana"));
    }

    #[test]
    fn an_empty_allowlist_ships_nothing() {
        let c = cfg(&[]);
        assert!(!WalTap::ships(&c, "edge-logs"));
    }

    #[test]
    fn actions_carry_external_versioning_so_redelivery_is_idempotent() {
        let mut out = String::new();
        let entry = WalEntry::Index {
            doc_id: "d1".into(),
            source: json!({"a": 1}),
        };
        assert_eq!(
            render_action("edge-logs", 42, &entry, &mut out),
            Some(false)
        );
        let meta: Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
        assert_eq!(meta["index"]["_id"], "d1");
        assert_eq!(meta["index"]["_index"], "edge-logs");
        assert_eq!(meta["index"]["version"], 42);
        assert_eq!(meta["index"]["version_type"], "external");
        assert_eq!(out.lines().nth(1).unwrap(), r#"{"a":1}"#);

        let mut out = String::new();
        let del = WalEntry::Delete {
            doc_id: "d1".into(),
        };
        assert_eq!(render_action("edge-logs", 43, &del, &mut out), Some(true));
        let meta: Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
        assert_eq!(meta["delete"]["version"], 43);
        assert_eq!(out.lines().count(), 1);
    }

    #[test]
    fn mapping_updates_render_nothing() {
        let mut out = String::new();
        let entry = WalEntry::UpdateMapping { schema: json!({}) };
        assert_eq!(render_action("i", 1, &entry, &mut out), None);
        assert!(out.is_empty());
    }

    #[test]
    fn unreadable_state_file_starts_from_now_rather_than_from_garbage() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(STATE_FILE), b"{not json").unwrap();
        let tap = WalTap::new(dir.path(), WalTapConfig::default());
        assert!(tap.cursors().is_empty());
    }

    #[test]
    fn backoff_is_capped_and_jittered() {
        let dir = tempfile::tempdir().unwrap();
        let config = WalTapConfig {
            poll_interval_ms: 500,
            max_retry_backoff_secs: 10,
            ..Default::default()
        };
        let tap = WalTap::new(dir.path(), config.clone());
        tap.arm_backoff(&config, 30);
        let armed = tap.retry_not_before_ms.load(Ordering::Relaxed);
        let delay = armed.saturating_sub(tap.now_ms());
        assert!(
            delay <= 10_000,
            "backoff must respect the cap, got {delay}ms"
        );
        assert!(
            delay >= 4_000,
            "backoff must still be substantial, got {delay}ms"
        );
    }
}
