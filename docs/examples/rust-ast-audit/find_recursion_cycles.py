#!/usr/bin/env python3
"""Find unguarded recursion cycles in the call graph.

This is the query a grep cannot express. A stack-overflow DoS in a recursive
descent parser is rarely direct self-recursion — it is a CYCLE:

    parse_qs_unary -> parse_qs_or -> parse_qs_and -> parse_qs_unary

No single file, function, or regex shows that. The call graph does: find the
strongly connected components of the internal call graph, then report the ones
where NO member establishes a depth bound. Each such cycle is a candidate
"attacker-controlled nesting depth becomes stack depth" bug.

Usage:
    python3 find_recursion_cycles.py <ast-out-dir> [--min-size 1] [--json]
"""

import argparse
import json
import os
import sys
from collections import defaultdict


def load(astdir):
    fns = [json.loads(l) for l in open(os.path.join(astdir, "functions.ndjson"))]
    calls = [json.loads(l) for l in open(os.path.join(astdir, "calls.ndjson"))]
    return fns, calls


def build_graph(fns, calls):
    """Edges between INTERNAL functions only, keyed by (file, name).

    Callee resolution is by short name within the same file first, then within
    the same crate, then globally unique name. Name collisions across crates are
    reported so the caller knows the graph is approximate there.
    """
    by_file_name = defaultdict(list)
    by_crate_name = defaultdict(list)
    by_name = defaultdict(list)
    for f in fns:
        key = f["id"]
        by_file_name[(f["file"], f["fn_name"])].append(key)
        by_crate_name[(f["crate"], f["fn_name"])].append(key)
        by_name[f["fn_name"]].append(key)

    info = {f["id"]: f for f in fns}
    caller_of = {f["id"]: f for f in fns}
    graph = defaultdict(set)
    ambiguous = 0

    skipped_method = 0
    for c in calls:
        src = c["caller_id"]
        if src not in caller_of:
            continue
        # Method calls are unresolvable without type inference. Resolving
        # `.len()` / `.push()` / `.new()` by bare name wires every same-named
        # function in the workspace together: measured, that collapsed 603
        # functions into ONE spurious strongly-connected component and hid the
        # real 3-cycle inside it. Free and path-qualified calls only.
        if c.get("is_method") or c["kind"] == "macro":
            skipped_method += 1
            continue
        f = caller_of[src]
        name = c["callee"]
        cands = (by_file_name.get((f["file"], name))
                 or by_crate_name.get((f["crate"], name))
                 or by_name.get(name))
        if not cands:
            continue
        if len(cands) > 1:
            ambiguous += 1
            # same-file candidates are the safest disambiguation
            same = [x for x in cands if info[x]["file"] == f["file"]]
            cands = same or cands[:1]
        for dst in cands[:1]:
            graph[src].add(dst)
    return graph, info, {"ambiguous": ambiguous, "skipped_method_calls": skipped_method}


def sccs(graph, nodes):
    """Tarjan, iterative (the graph is large enough that recursion would be ironic)."""
    index = {}
    low = {}
    on_stack = {}
    stack = []
    result = []
    counter = [0]

    for root in nodes:
        if root in index:
            continue
        work = [(root, iter(graph.get(root, ())))]
        index[root] = low[root] = counter[0]
        counter[0] += 1
        stack.append(root)
        on_stack[root] = True

        while work:
            node, it = work[-1]
            advanced = False
            for nxt in it:
                if nxt not in index:
                    index[nxt] = low[nxt] = counter[0]
                    counter[0] += 1
                    stack.append(nxt)
                    on_stack[nxt] = True
                    work.append((nxt, iter(graph.get(nxt, ()))))
                    advanced = True
                    break
                elif on_stack.get(nxt):
                    low[node] = min(low[node], index[nxt])
            if advanced:
                continue
            work.pop()
            if work:
                parent = work[-1][0]
                low[parent] = min(low[parent], low[node])
            if low[node] == index[node]:
                comp = []
                while True:
                    w = stack.pop()
                    on_stack[w] = False
                    comp.append(w)
                    if w == node:
                        break
                result.append(comp)
    return result


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("astdir")
    ap.add_argument("--min-size", type=int, default=1)
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args()

    fns, calls = load(args.astdir)
    graph, info, res = build_graph(fns, calls)
    comps = sccs(graph, list(info.keys()))

    cycles = []
    for comp in comps:
        # size 1 is a cycle only if the function calls itself
        if len(comp) == 1 and comp[0] not in graph.get(comp[0], ()):
            continue
        members = [info[c] for c in comp]
        if any(m["is_test"] for m in members):
            continue
        guarded = any(m["has_depth_guard"] for m in members)
        cycles.append({
            "size": len(comp),
            "guarded": guarded,
            "crates": sorted({m["crate"] for m in members}),
            "files": sorted({m["file"] for m in members}),
            "members": [
                {"fn": m["fn_name"], "file": m["file"], "line": m["line_start"],
                 "has_depth_guard": m["has_depth_guard"], "is_pub": m["is_pub"]}
                for m in sorted(members, key=lambda m: (m["file"], m["line_start"]))
            ],
        })

    cycles.sort(key=lambda c: (c["guarded"], -c["size"]))
    unguarded = [c for c in cycles if not c["guarded"]]
    out = {
        "total_functions": len(fns),
        "call_edges": len(calls),
        "ambiguous_callee_resolutions": res["ambiguous"],
        "skipped_method_calls": res["skipped_method_calls"],
        "recursion_cycles": len(cycles),
        "unguarded_cycles": len(unguarded),
        "cycles": [c for c in cycles if c["size"] >= args.min_size],
    }

    if args.json:
        print(json.dumps(out, indent=2))
        return

    print(f"functions={out['total_functions']} edges={out['call_edges']} "
          f"cycles={out['recursion_cycles']} unguarded={out['unguarded_cycles']} "
          f"(ambiguous: {res['ambiguous']}, method calls skipped: {res['skipped_method_calls']})")
    for c in cycles:
        if c["size"] < args.min_size:
            continue
        tag = "GUARDED" if c["guarded"] else "UNGUARDED"
        print(f"\n[{tag}] cycle of {c['size']} in {','.join(c['crates'])}")
        for m in c["members"]:
            g = " (depth guard)" if m["has_depth_guard"] else ""
            print(f"    {m['file']}:{m['line']} {m['fn']}{g}")


if __name__ == "__main__":
    main()
