# Reproduce the measured fused FP32 MiniLM graph

This offline recipe transforms one exact
`sentence-transformers/all-MiniLM-L6-v2`-compatible ONNX export into the
transformer-fused FP32 graph measured by XERJ's FB20 optimizer-regression
screen. It does **not** download, commit, install, or select a model. The
approximately 90 MB source and output remain external runtime assets.

The recipe is intentionally narrow and fail-closed:

- CPython 3.13.5 and every optimizer package are exactly version-pinned; every
  selected wheel is SHA-256 pinned;
- source model and tokenizer must match measured lengths and SHA-256 hashes;
- the result must match the measured bytes, hash, interface, opsets,
  initializer types, and complete operator inventory;
- a lock is acquired with `O_EXCL`;
- candidate and manifest are staged and synced inside one private sibling
  directory, then the complete directory is atomically published without
  replacement;
- any mismatch exits non-zero without publishing an artifact.

The final output directory does not become visible until both files are
complete. A process or machine failure may leave a hidden sibling staging
directory or lock, but cannot publish a partial final directory. First confirm
that the PID in `.<output-directory>.optimize.lock` is no longer alive; then
remove the hidden staging directory and stale lock before rerunning. Never
delete a live lock.

## Locked environment

The lock targets ordinary CPython 3.13.5 on GNU/Linux glibc x86-64, the only
generation environment used for the exact byte result. It is not a general
cross-platform Python lock.

```bash
python3.13 -m venv /path/to/xerj-onnx-recipe-venv
/path/to/xerj-onnx-recipe-venv/bin/python -m pip install \
  --only-binary=:all: \
  --require-hashes \
  -r demo/playbooks/onnx-model-optimization/requirements-linux-x86_64-cp313.lock
```

## Reproduce

```bash
/path/to/xerj-onnx-recipe-venv/bin/python \
  demo/playbooks/onnx-model-optimization/optimize_minilm_fp32.py \
  --source /models/minilm-source/model.onnx \
  --tokenizer /models/minilm-source/tokenizer.json \
  --output-dir /models/minilm-fused-v1
```

The published directory contains `model.bert-fused-fp32.onnx` and
`model.bert-fused-fp32.manifest.json`.

Required inputs:

| Asset | Bytes | SHA-256 |
|---|---:|---|
| source model | 90,405,214 | `6fd5d72fe4589f189f8ebc006442dbb529bb7ce38f8082112682524616046452` |
| tokenizer | 466,247 | `be50c3628f2bf5bb5e3a7f17b1f74611b2561a3a27eeab05e5aa30f411572037` |

Required result:

| Asset | Bytes | SHA-256 |
|---|---:|---|
| fused model | 90,276,689 | `d45c738311abe1488b6d9e2862e1db08267fc31d6f301d8f26163e2fb4956b9c` |

The manifest embeds the complete checked-in artifact contract and records the
observed source and candidate graph inventories plus the exact toolchain. The
recipe compares the complete normalized observation to the contract before it
publishes anything. The contract also records that the
original experiment did not preserve an immutable upstream export revision:
the exact source bytes are pinned, but a stronger upstream provenance claim
would be dishonest.

[`artifact-contract.json`](artifact-contract.json) is the checked-in expected
contract. The generated manifest adds the complete inspected source and
candidate inventories and must agree with it.

## Use with XERJ

Build and start XERJ as described in
[`docs/EXPERIMENTAL_ONNX.md`](../../../docs/EXPERIMENTAL_ONNX.md), using the
generated model:

```bash
target/release/xerj \
  --insecure \
  --data-dir /var/lib/xerj-fused \
  --embed-mode onnx-experimental \
  --onnx-model /models/minilm-fused-v1/model.bert-fused-fp32.onnx \
  --onnx-tokenizer /models/minilm-source/tokenizer.json

target/release/xerj autoindex /path/to/corpus \
  --url http://localhost:9200 \
  --prefix finance-minilm-fused-v1 \
  --fresh
```

The fused graph is a **new embedding identity**. Its output is numerically
close to the source graph but not bit-identical. Never resume an index created
with the source model under the fused model. Retain the old asset for the old
index, or use `autoindex --fresh` with a new prefix. Do not bypass
`embedding_identity.json`.

## Compatibility boundary

This is offline transformer fusion (`opt_level=0`), not a hardware-specific
ONNX Runtime optimized-session cache. The graph nevertheless contains
`com.microsoft` contrib operators. The proven product path is ONNX Runtime
transformer optimizer 1.22.1 for generation, then XERJ's `ort`/`ort-sys`
2.0.0-rc.12 API-24 CPU path on GNU/Linux glibc x86-64. The generation-tool
version is not a claim that XERJ consumes the graph through runtime 1.22.1.

Do not infer support for another runtime/API line, CPU architecture, execution
provider, musl build, or non-ORT consumer. Even where the graph loads,
different kernels or CPUs can change floating-point output. Run the
asset/runtime and retrieval gates before expanding the support matrix.

## Evidence boundary

This recipe was extracted from a promising private optimizer-regression
screen, but the branch intentionally contains neither that corpus nor its raw
evidence. Therefore this playbook makes no public speed or retrieval-quality
claim. A publication-grade claim still requires the repository's ten-arm
comparison contract, checked-in sanitized evidence, the full quality gate, and
the sealed 368-PDF North Star.

Run focused tests without invoking the 90 MB optimizer:

```bash
PYTHONPATH=/path/to/xerj-onnx-recipe-venv/lib/python3.13/site-packages \
  python3.13 -m unittest \
  demo/playbooks/onnx-model-optimization/test_optimize_minilm_fp32.py
```
