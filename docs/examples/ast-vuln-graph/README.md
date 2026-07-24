# AST + graph vulnerability finding (prototype)

Finds interprocedural vulnerabilities in PHP/WordPress that per-file tools miss,
and hands an AI only the code on each taint path — far fewer tokens. Full
write-up: [`docs/research/ast-graph-vuln-detection.md`](../../research/ast-graph-vuln-detection.md).

## Run it

```bash
pip install phply
python3 make_demo_plugin.py ./acme-plugin    # 6 planted vulns among ~60 benign files
python3 extract_ast.py       ./acme-plugin    # AST -> taint graph -> findings
```

Expected: **6/6** vulnerabilities, including the interprocedural SQLi
(`acme_ajax_search -> acme_find_items`, unauthenticated), the interprocedural
XSS, and the missing-nonce CSRF — none of which a single-file scan produces.
`ast_facts.json` holds the per-function facts (index these into XERJ for the
FTS/semantic navigation layer described in the research doc).

## What each file does

- `make_demo_plugin.py` — reproducible vulnerable WordPress plugin (ground truth).
- `extract_ast.py` — parses PHP with phply, builds per-function taint facts and
  the call graph, and reports interprocedural source→sink findings + CSRF.

This is triage: it surfaces real, reachable patterns so an AI can confirm
exploitability with minimal context. See the research doc for the honest limits.
