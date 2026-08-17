"""Index a small slice of the rust-text corpus into a XERJ index whose `body`
field is `semantic_text`, so the server's configured embedding backend embeds
it on ingest.

usage: ingest_slice.py <base_url> <index_name>
"""

import json
import os
import sys
import time
import urllib.error
import urllib.request

SLICE_ROOT = "/home/claude/.xerj-code/corpora/rust-text/regex/regex-automata"

MAPPING = {
    "mappings": {
        "properties": {
            "title": {"type": "keyword"},
            "path": {"type": "keyword"},
            "language": {"type": "keyword"},
            "body": {"type": "semantic_text"},
        }
    }
}


def req(method, url, payload=None, ctype="application/json"):
    data = payload.encode() if isinstance(payload, str) else payload
    r = urllib.request.Request(url, data=data, method=method,
                               headers={"Content-Type": ctype})
    try:
        with urllib.request.urlopen(r, timeout=1800) as resp:
            return resp.status, resp.read().decode()
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode()


def main():
    base, index = sys.argv[1], sys.argv[2]

    print(f"DELETE {index}: {req('DELETE', f'{base}/{index}')[0]}")
    status, body = req("PUT", f"{base}/{index}", json.dumps(MAPPING))
    print(f"PUT {index}: {status} {body[:300]}")
    if status >= 300:
        sys.exit(1)

    files = []
    for root, _dirs, names in os.walk(SLICE_ROOT):
        for name in sorted(names):
            if name.endswith(".rs"):
                files.append(os.path.join(root, name))
    files.sort()
    print(f"slice: {len(files)} .rs files under {SLICE_ROOT}")

    lines = []
    total_bytes = 0
    for path in files:
        with open(path, "r", encoding="utf-8", errors="replace") as fh:
            text = fh.read()
        total_bytes += len(text)
        doc = {
            "title": os.path.basename(path),
            "path": os.path.relpath(path, SLICE_ROOT),
            "language": "rust",
            "body": text,
        }
        lines.append(json.dumps({"index": {"_index": index}}))
        lines.append(json.dumps(doc))
    print(f"slice bytes: {total_bytes}")

    # Bulk in small batches so a slow CPU model does not blow the HTTP timeout.
    batch, sent, t0 = 8, 0, time.time()
    for i in range(0, len(lines), batch * 2):
        payload = "\n".join(lines[i:i + batch * 2]) + "\n"
        status, body = req("POST", f"{base}/_bulk", payload,
                           ctype="application/x-ndjson")
        if status >= 300:
            print(f"bulk FAILED {status}: {body[:600]}")
            sys.exit(1)
        parsed = json.loads(body)
        if parsed.get("errors"):
            for item in parsed.get("items", []):
                err = list(item.values())[0].get("error")
                if err:
                    print(f"item error: {json.dumps(err)[:500]}")
                    sys.exit(1)
        sent += batch
        print(f"  indexed ~{min(sent, len(files))}/{len(files)}  "
              f"({time.time() - t0:.1f}s)", flush=True)

    req("POST", f"{base}/{index}/_refresh")
    status, body = req("GET", f"{base}/{index}/_count")
    print(f"count: {body}")
    print(f"total ingest wall time: {time.time() - t0:.1f}s")


if __name__ == "__main__":
    main()
