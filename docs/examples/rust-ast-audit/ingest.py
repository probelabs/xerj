#!/usr/bin/env python3
"""Index the Rust AST substrate into XERJ (ES-compatible wire).

Creates three indices with EXPLICIT mappings. Explicit mappings matter here:
the audit filters on exact values (crate names, sink categories, op names), so
those fields must be `keyword`, not analyzed `text`. Only the code bodies and
signatures are `text` (for phrase search inside code).

Usage:
    python3 ingest.py <ast-out-dir> [--url http://127.0.0.1:9310] [--suffix ""]
"""

import argparse
import json
import os
import sys
import time
import urllib.error
import urllib.request

KEYWORD = {"type": "keyword"}
TEXT = {"type": "text"}
INT = {"type": "integer"}
BOOL = {"type": "boolean"}

FUNCTIONS_MAPPING = {
    "mappings": {
        "properties": {
            "id": KEYWORD,
            "crate": KEYWORD,
            "file": KEYWORD,
            "line_start": INT,
            "line_end": INT,
            "fn_name": KEYWORD,
            "module_path": KEYWORD,
            "owner": KEYWORD,
            "trait_impl": KEYWORD,
            "in_unsafe_impl": BOOL,
            "is_test": BOOL,
            "signature": TEXT,
            "params": TEXT,
            "param_names": KEYWORD,
            "return_type": TEXT,
            "is_async": BOOL,
            "is_unsafe_fn": BOOL,
            "is_pub": BOOL,
            "loc": INT,
            "has_unsafe_block": BOOL,
            "unsafe_block_count": INT,
            "unsafe_block_lines": INT,
            "unsafe_ops": KEYWORD,
            "unsafe_any": BOOL,
            "panic_ops": KEYWORD,
            "panic_count": INT,
            "index_expr_count": INT,
            "index_exprs": TEXT,
            "cast_count": INT,
            "casts": TEXT,
            "narrowing_casts": TEXT,
            "narrowing_cast_count": INT,
            "has_narrowing_cast": BOOL,
            "alloc_ops": KEYWORD,
            "alloc_from_param_count": INT,
            "alloc_args": TEXT,
            "alloc_param_names": KEYWORD,
            "alloc_product": BOOL,
            "alloc_args_all": TEXT,
            "guard_after_destructive_op": BOOL,
            "path_join_args": TEXT,
            "path_join_from_param": BOOL,
            "calls_self": BOOL,
            "has_depth_guard": BOOL,
            "reads_config_limit": BOOL,
            "extractors": KEYWORD,
            "is_handler_shaped": BOOL,
            "sinks": KEYWORD,
            "validators": KEYWORD,
            "concurrency": KEYWORD,
            "lock_across_await": BOOL,
            "body": TEXT,
            "body_truncated": BOOL,
            "body_chars": INT,
            "body_lines": INT,
        }
    }
}

CALLS_MAPPING = {
    "mappings": {
        "properties": {
            "caller_id": KEYWORD,
            "caller": KEYWORD,
            "file": KEYWORD,
            "line": INT,
            "callee_path": TEXT,
            "callee": KEYWORD,
            "kind": KEYWORD,
            "is_method": BOOL,
            "resolvable": BOOL,
        }
    }
}

ROUTES_MAPPING = {
    "mappings": {
        "properties": {
            "file": KEYWORD,
            "crate": KEYWORD,
            "line": INT,
            "method": KEYWORD,
            "path": KEYWORD,
            "handler": KEYWORD,
            "handler_path": TEXT,
            "unauth_looking": BOOL,
        }
    }
}


def req(url, method="GET", body=None, ctype="application/json"):
    data = body.encode() if isinstance(body, str) else body
    r = urllib.request.Request(url, data=data, method=method)
    r.add_header("Content-Type", ctype)
    try:
        with urllib.request.urlopen(r, timeout=300) as resp:
            return resp.status, resp.read().decode()
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode()


def create(url, name, mapping):
    req(f"{url}/{name}", "DELETE")
    st, body = req(f"{url}/{name}", "PUT", json.dumps(mapping))
    if st >= 300:
        sys.exit(f"create {name} failed: {st} {body[:500]}")
    return st


def bulk_load(url, index, path, batch=2000):
    """NDJSON -> _bulk in batches. `alloc_from_param` is dropped: it is a list of
    objects and the audit queries it via alloc_ops/param names instead."""
    n = 0
    t0 = time.time()
    buf = []
    errors = []

    def flush():
        nonlocal buf, n
        if not buf:
            return
        payload = "".join(buf)
        st, body = req(f"{url}/{index}/_bulk", "POST", payload,
                       "application/x-ndjson")
        if st >= 300:
            errors.append(f"{st}: {body[:300]}")
        else:
            d = json.loads(body)
            if d.get("errors"):
                for item in d.get("items", []):
                    act = next(iter(item.values()))
                    if act.get("error"):
                        errors.append(json.dumps(act["error"])[:300])
                        break
        buf = []

    for line in open(path):
        rec = json.loads(line)
        rec.pop("alloc_from_param", None)
        buf.append(json.dumps({"index": {"_index": index}}) + "\n")
        buf.append(json.dumps(rec, separators=(",", ":")) + "\n")
        n += 1
        if n % batch == 0:
            flush()
    flush()
    req(f"{url}/{index}/_refresh", "POST")
    return n, round(time.time() - t0, 2), errors[:5]


def count(url, index):
    st, body = req(f"{url}/{index}/_count")
    if st >= 300:
        return -1
    return json.loads(body).get("count", -1)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("astdir")
    ap.add_argument("--url", default="http://127.0.0.1:9310")
    ap.add_argument("--suffix", default="")
    args = ap.parse_args()

    sfx = args.suffix
    plan = [
        (f"rustfns{sfx}", FUNCTIONS_MAPPING, "functions.ndjson"),
        (f"rustcalls{sfx}", CALLS_MAPPING, "calls.ndjson"),
        (f"rustroutes{sfx}", ROUTES_MAPPING, "routes.ndjson"),
    ]
    out = {"url": args.url, "indices": {}}
    for name, mapping, fname in plan:
        create(args.url, name, mapping)
        path = os.path.join(args.astdir, fname)
        sent, secs, errs = bulk_load(args.url, name, path)
        got = count(args.url, name)
        out["indices"][name] = {
            "sent": sent, "indexed": got, "match": sent == got,
            "seconds": secs, "sample_errors": errs,
        }
    print(json.dumps(out, indent=2))
    if not all(v["match"] for v in out["indices"].values()):
        sys.exit("COUNT MISMATCH — investigate before trusting the index")


if __name__ == "__main__":
    main()
