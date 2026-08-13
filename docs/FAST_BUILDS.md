# Building XERJ faster, and using less of the machine

Written 2026-08-12 after three agents cold-building this workspace in parallel
drove a 32-core box to load 92 and still hadn't produced a binary three hours
later. Every lever below is ordered by measured or expected impact. Numbers
marked ESTIMATED are not yet measured on this workspace — do not quote them
anywhere public until they are (see *Measuring* at the end).

## The one that matters: stop paying shipping-grade optimisation to verify a fix

`[profile.release]` is `lto = "fat"` + `codegen-units = 1`. That is deliberately
the slowest build Rust can produce, because it yields the fastest binary. Fat
LTO runs largely **single-threaded** at link time and `codegen-units = 1`
removes intra-crate parallelism — which is why you see one `rustc` pinned at
100% for twenty minutes on a 32-core machine while nothing else progresses.

Correct for an artifact users download. Wrong for "does my one-line fix
compile?", where the binary's runtime speed is irrelevant to the question.

Use the `quick` profile (defined in `engine/Cargo.toml`):

```sh
cargo build --profile quick -j 12 -p xerj-server
cargo test  --profile quick -j 12 -p xerj-autoindex
```

Its sibling `[profile.ci-test]` serves `cargo test`; use `quick` for `cargo build`.

It keeps everything affecting **correctness** identical to release —
`panic = "abort"` (inherited; it changes observable behaviour so it must match),
same features, same `target-cpu` — and relaxes only optimisation.

- **Valid for:** does it compile, do tests pass, is ES-YAML still 0 failed, does
  the fix work.
- **NEVER valid for:** benchmarks, latency, published numbers, release
  artifacts. Those stay `--release`. This is an honest-claims rule, not a style
  preference — a `quick` binary is materially slower and quoting a number from
  one would be false.

## Do not build the same 666 crates N times

This workspace has 16 crates and **666 dependency packages**. Cargo keys
artifacts by absolute path, so **every git worktree gets its own cold
`target/`** and recompiles all 666. Three isolated agents = three full builds of
identical dependencies. Individual crates here cost real money: `xerj_api` was
measured at 17–24 minutes of CPU, `pdf_oxide` at 24 minutes — each, each time.

Options, best first:

1. **`sccache`** — a compilation cache shared across target dirs, so N worktrees
   compile the dep graph once. Not currently installed.
   ```sh
   cargo install sccache          # or: apt/dnf install sccache
   export RUSTC_WRAPPER=sccache
   sccache --show-stats           # verify hits are actually happening
   ```
   This is the direct fix for the worktree problem and needs no change to how
   agents are launched.
2. **Shared `CARGO_TARGET_DIR`** — builds the dep graph once, but cargo takes an
   exclusive lock, so concurrent agents *serialise*. Better for total CPU, worse
   for one agent's wall-clock. See the shared-target-dir mtime trap noted in the
   issue-270 work before relying on it for timing-sensitive verification.
3. **Don't isolate agents that don't need it.** Worktrees exist for agents that
   mutate the *same* files. Agents touching disjoint crates can share a tree —
   but then never switch branches under a peer, and stage explicit paths rather
   than `git add -A`.

## Link faster

`wild` is already installed at `~/.cargo/bin/wild` and unused; `clang` is
available. The default GNU `ld` is the slowest option, and link time is a large
share of every incremental rebuild.

Opt in per-shell rather than globally, so release and cross-compile builds are
untouched:

```sh
export RUSTFLAGS="-C link-arg=-fuse-ld=$HOME/.cargo/bin/wild"
```

ESTIMATED win: large on incremental rebuilds, ~none on a cold build dominated by
codegen. Note it compounds with `lto = false` and does almost nothing under
`lto = "fat"`, where the LTO pass dominates. Keep it out of
`.cargo/config.toml`: `wild` is young, and a linker bug in a *released* binary
is a far worse outcome than a slow build.

## Stop compiling C we already have

`zstd-sys` and `libsqlite3-sys` build bundled C on every cold build — that is
what the long-running `cc1` processes are. The system already has zstd **1.5.7**
and sqlite3 **3.46.1**:

```sh
export ZSTD_SYS_USE_PKG_CONFIG=1
export LIBSQLITE3_SYS_USE_PKG_CONFIG=1
```

Dev shells only. Release artifacts must keep the vendored copies — that is what
makes the shipped binary portable across machines that lack these libraries, and
it is a deliberate property of the single-static-binary promise, not an
oversight.

Other C-building deps worth knowing about: `ort-sys` (ONNX runtime, the neural
embedder), `tikv-jemalloc-sys`, `openssl-sys`, `ring`.

## Parallelise the compiler frontend

A nightly toolchain is installed. The parallel frontend splits work *within* a
single crate, which is exactly what helps the long-pole crates that stall the
build graph while 30 cores idle:

```sh
cargo +nightly build --profile quick -Zthreads=8 -p xerj-server
```

Nightly-only, verification-only. Never for released artifacts.

## Pick `-j` for the machine, not the flag

`-j 32` per build is correct for **one** build. It is wrong for three: 3 × 32 on
32 cores produced load 92, and each build ran roughly 4–5× slower than it would
have alone — one produced 27 dependency artifacts in two hours. Running them
sequentially would have finished all three sooner. When N builds share the box,
use `-j (cores / N)`, or better, don't run N builds.

## Longer-term: shrink the graph

666 packages for 16 crates is the root cost behind every cold build. Not a
quick fix, but worth an audit: `cargo tree --duplicates` finds crates compiled
at multiple versions, and feature unification often pulls in more than intended.
`cargo bloat` / `cargo llvm-lines` identify crates whose codegen dominates.

## Measuring, before anyone quotes a number

Nothing above has been benchmarked on this workspace yet — the machine was
saturated when it was written, and measuring a build on a contended box produces
a number that says more about the contention than the change.

```sh
cargo build --profile quick -p xerj-server --timings   # writes target/cargo-timings/
```

Run each arm on an otherwise-idle machine, twice, discarding the first.
Per this repo's honest-claims rules, no build-speed number goes in a commit
message, doc, or public page until it comes from such a run.
