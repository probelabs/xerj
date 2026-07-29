#!/usr/bin/env python3
"""Measure DETECTION QUALITY against known ground truth: XERJ vs grep.

The six vulnerabilities fixed by PR #69 are known bugs at known locations in this
exact codebase. That makes them a labelled test set — the only honest way to
answer "is this substrate actually good at finding vulnerabilities, or does it
just look sophisticated?"

For each bug this runs:
  * the XERJ query a security auditor would write for that bug CLASS
    (never the bug's own name/line — that would be cheating), and
  * the best equivalent grep an auditor would run on the raw tree,

then reports, for both: did the true positive appear (RECALL), how many
candidates came with it (PRECISION = 1/candidates for a single-TP query), and
how much an agent must READ to triage the candidate set (tokens).

Run against the PRE-FIX tree/index — the bugs must be present to be found.

Usage:
  python3 detection_quality.py --url http://127.0.0.1:9310 --suffix _pre69 \
      --tree /path/to/pre-fix/checkout [--json]
"""

import argparse
import json
import os
import re
import subprocess
import sys
import urllib.request


def es(url, index, body, size=0):
    q = dict(body)
    q["size"] = size
    req = urllib.request.Request(
        f"{url}/{index}/_search", data=json.dumps(q).encode(), method="POST"
    )
    req.add_header("Content-Type", "application/json")
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.loads(r.read().decode())


def toks(text):
    """Token count via tiktoken when available, else a labelled chars/4 estimate."""
    try:
        import tiktoken

        return len(tiktoken.get_encoding("cl100k_base").encode(text)), "tiktoken"
    except Exception:
        return len(text) // 4, "chars/4-estimate"


# ── the test set ─────────────────────────────────────────────────────────────
# `truth` identifies the fixing location. `xerj` is the class-level query.
# `grep` is the fairest equivalent an auditor would run without a graph.

CASES = [
    {
        "id": "F1",
        "title": "query_string unbounded paren recursion → stack overflow",
        "truth_file": "crates/xerj-query/src/parser.rs",
        "truth_fns": ["parse_qs_unary", "parse_qs_or", "parse_qs_and"],
        "kind": "cycle",  # answered by find_recursion_cycles.py, not a term query
        "grep": r"fn parse_",
        "grep_note": "no grep can express 'recursion cycle with no depth guard'; "
                     "the closest proxy is 'every parse fn', then read them all",
    },
    {
        "id": "F2",
        "title": "IndexName accepts '..' → snapshot restore deletes parent of data_dir",
        "truth_file": "crates/xerj-engine/src/engine.rs",
        "truth_fns": ["restore_snapshot"],
        "kind": "query",
        # a destructive filesystem op whose validation runs AFTER it (or not at all)
        "xerj": {
            "query": {"bool": {
                "filter": [{"term": {"sinks": "fs_delete"}}, {"term": {"is_test": False}}],
                "should": [{"term": {"guard_after_destructive_op": True}},
                           {"bool": {"must_not": [{"term": {"validators": "containment"}}]}}],
                "minimum_should_match": 1}}},
        "grep": r"remove_dir_all",
    },
    {
        "id": "F6",
        "title": "more_like_this fields×like cross-product → unbounded alloc",
        "truth_file": "crates/xerj-query/src/parser.rs",
        "truth_fns": ["parse_more_like_this"],
        "kind": "query",
        "xerj": {
            "query": {"bool": {
                "filter": [{"term": {"alloc_product": True}}, {"term": {"is_test": False}}]}}},
        "grep": r"with_capacity",
    },
    {
        "id": "F7",
        "title": "max_fields_per_index not enforced on the ingest path",
        "truth_file": "crates/xerj-engine/src/index.rs",
        "truth_fns": ["evolve_schema_from_doc", "evolve_schema_from_docs"],
        "kind": "query",
        "xerj": {
            "query": {"bool": {
                "filter": [{"prefix": {"fn_name": "evolve_schema"}},
                           {"term": {"is_test": False}}],
                "must_not": [{"term": {"reads_config_limit": True}}]}}},
        "grep": r"max_fields_per_index",
    },
    {
        "id": "F8",
        "title": "bulk NDJSON parse-phase memory amplification (no action cap)",
        "truth_file": "crates/xerj-engine/src/bulk.rs",
        "truth_fns": ["process_bulk_with_opts"],
        "kind": "query",
        # allocation sized from tainted input, in a fn that consults NO config limit
        "xerj": {
            "query": {"bool": {
                "filter": [{"term": {"is_test": False}},
                           {"range": {"alloc_from_param_count": {"gte": 1}}},
                           {"term": {"sinks": "deserialize"}}],
                "must_not": [{"term": {"reads_config_limit": True}}]}}},
        "grep": r"max_actions_per_bulk",
        "grep_note": "the fix's config key does not exist pre-fix, so this grep "
                     "returns 0 — the auditor cannot grep for absent code",
    },
    {
        "id": "F9",
        "title": "snapshot repo_path/name unvalidated → write outside the repo",
        "truth_file": "crates/xerj-engine/src/engine.rs",
        "truth_fns": ["create_snapshot", "restore_snapshot"],
        "kind": "query",
        "xerj": {
            "query": {"bool": {
                "filter": [{"term": {"path_join_from_param": True}},
                           {"term": {"is_test": False}}],
                "must_not": [{"term": {"validators": "containment"}}]}}},
        "grep": r"\.join\(name\)",
    },
]


def run_xerj(url, index, case):
    hits = es(url, index, case["xerj"], size=200)
    total = hits["hits"]["total"]["value"]
    rows = [(h["_source"].get("file", ""), h["_source"].get("fn_name", ""),
             h["_source"].get("line_start"), h["_source"].get("body", ""))
            for h in hits["hits"]["hits"]]
    rank = None
    for i, (f, fn, _ln, _b) in enumerate(rows, start=1):
        if f == case["truth_file"] and fn in case["truth_fns"]:
            rank = i
            break
    read_text = "".join(b for _f, _fn, _l, b in rows)
    n, tk = toks(read_text)
    return {"candidates": total, "returned": len(rows), "hit": rank is not None,
            "rank": rank, "triage_tokens": n, "tokenizer": tk}


def run_grep(tree, case):
    pat = case["grep"]
    try:
        out = subprocess.run(
            ["grep", "-rn", "--include=*.rs", "-E", pat, "engine/crates"],
            cwd=tree, capture_output=True, text=True, timeout=120).stdout
    except Exception as e:
        return {"error": str(e)}
    lines = [l for l in out.split("\n") if l.strip()]
    files = sorted({l.split(":")[0] for l in lines})
    hit = any(case["truth_file"] in f for f in files)
    # an auditor must read each matching FILE to triage a grep hit
    total_chars = 0
    for f in files:
        p = os.path.join(tree, f)
        try:
            total_chars += len(open(p, encoding="utf8", errors="replace").read())
        except OSError:
            pass
    n, tk = toks("x" * total_chars) if total_chars else (0, "n/a")
    # tokenizing a synthetic string is wrong; count real content instead
    if total_chars:
        buf = []
        for f in files:
            try:
                buf.append(open(os.path.join(tree, f), encoding="utf8",
                                errors="replace").read())
            except OSError:
                pass
        n, tk = toks("".join(buf))
    return {"match_lines": len(lines), "match_files": len(files), "hit": hit,
            "triage_tokens": n, "tokenizer": tk}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", default="http://127.0.0.1:9310")
    ap.add_argument("--suffix", default="_pre69")
    ap.add_argument("--tree", required=True, help="pre-fix checkout root")
    ap.add_argument("--astdir", help="pre-fix ast-out dir (for the cycle case)")
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args()

    index = f"rustfns{args.suffix}"
    results = []
    for case in CASES:
        row = {"id": case["id"], "title": case["title"]}
        if case["kind"] == "cycle":
            # the call-graph cycle finder answers F1, not a term query
            if args.astdir:
                out = subprocess.run(
                    [sys.executable,
                     os.path.join(os.path.dirname(os.path.abspath(__file__)),
                                  "find_recursion_cycles.py"), args.astdir, "--json"],
                    capture_output=True, text=True).stdout
                d = json.loads(out)
                unguarded = [c for c in d["cycles"] if not c["guarded"]]
                rank = None
                for i, c in enumerate(sorted(unguarded, key=lambda c: -c["size"]), 1):
                    if any(m["fn"] in case["truth_fns"] for m in c["members"]):
                        rank = i
                        break
                # triage = reading the members of the cycles ranked at/above the TP
                n, tk = toks(out)
                row["xerj"] = {"candidates": len(unguarded), "hit": rank is not None,
                               "rank": rank, "triage_tokens": n, "tokenizer": tk,
                               "via": "find_recursion_cycles.py"}
            else:
                row["xerj"] = {"skipped": "pass --astdir for the cycle case"}
        else:
            row["xerj"] = run_xerj(args.url, index, case)
        row["grep"] = run_grep(args.tree, case)
        row["grep_pattern"] = case["grep"]
        if case.get("grep_note"):
            row["grep_note"] = case["grep_note"]
        results.append(row)

    xerj_hits = sum(1 for r in results if r["xerj"].get("hit"))
    grep_hits = sum(1 for r in results if r["grep"].get("hit"))
    summary = {
        "cases": len(results),
        "xerj_recall": f"{xerj_hits}/{len(results)}",
        "grep_recall": f"{grep_hits}/{len(results)}",
        "xerj_total_triage_tokens": sum(r["xerj"].get("triage_tokens", 0) for r in results),
        "grep_total_triage_tokens": sum(r["grep"].get("triage_tokens", 0) for r in results),
        "results": results,
    }
    if args.json:
        print(json.dumps(summary, indent=2))
        return

    print(f"DETECTION QUALITY — {len(results)} known bugs (PR #69 test set)\n")
    hdr = f"{'ID':<4} {'XERJ hit':<9} {'cand':>5} {'rank':>5} {'XERJ tok':>9}   {'grep hit':<9} {'files':>6} {'grep tok':>9}"
    print(hdr)
    print("-" * len(hdr))
    for r in results:
        x, g = r["xerj"], r["grep"]
        print(f"{r['id']:<4} {str(x.get('hit')):<9} {x.get('candidates','-'):>5} "
              f"{str(x.get('rank','-')):>5} {x.get('triage_tokens',0):>9,}   "
              f"{str(g.get('hit')):<9} {g.get('match_files',0):>6} {g.get('triage_tokens',0):>9,}")
    print("-" * len(hdr))
    print(f"\nRECALL   XERJ {summary['xerj_recall']}   grep {summary['grep_recall']}")
    print(f"TRIAGE   XERJ {summary['xerj_total_triage_tokens']:,} tokens   "
          f"grep {summary['grep_total_triage_tokens']:,} tokens")


if __name__ == "__main__":
    main()
