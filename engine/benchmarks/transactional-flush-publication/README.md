# Transactional flush publication A/B evidence

This directory preserves the exact local A/B harness and raw evidence used for the PR description.

Compared revisions:

- Base: `761b47dfcb5f8fcc57bc929385c422bcffbe130b`
- Head: `e601555b0eb8a371ba7d9e2eb3edc0066d63e503`

Run order was base followed by head on the same otherwise-idle host. Each run used a new data directory, the same 1,000-document mapping and corpus, one persistent HTTP connection, a 1,000-GET correctness-checked warmup, five trials of 20,000 correctness-checked GETs, and 30 write-plus-explicit-refresh operations per trial. Every measured GET parsed the response and checked `found` and the expected source value. Every refresh was followed by a GET checking the newly written source. Initial and final counts were also checked.

Build and reproduce from the repository root:

```bash
git worktree add ../xerj-flush-base 761b47dfcb5f8fcc57bc929385c422bcffbe130b
(cd ../xerj-flush-base/engine && cargo build --release -j 32 -p xerj-server)
(cd engine && cargo build --release -j 32 -p xerj-server)
cd engine/benchmarks/transactional-flush-publication
./run_one.sh base ../../../../xerj-flush-base/engine/target/release/xerj "$(mktemp -d)" base.jsonl
./run_one.sh head ../../target/release/xerj "$(mktemp -d)" head.jsonl
./summarize.py
```

Artifacts:

- `run_one.sh`: executable server and client harness.
- `summarize.py`: median and percentage-difference calculation.
- `environment.txt`: revisions, normalized binary SHA-256 labels, kernel, Rust toolchain, and memory.
- `base.jsonl`, `head.jsonl`: five raw trials per build.
- `summary.json`: mechanically computed medians and differences.

The comparison is sequential, single-host, and short. The results support only the conclusion that this probe found no material point-read or explicit-refresh regression. They do not demonstrate a speedup.
