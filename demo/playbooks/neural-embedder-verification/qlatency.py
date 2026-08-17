"""Per-query wall time for a `semantic` query, which includes embedding the
query text with whichever backend the server runs. Every iteration uses a
DISTINCT query string so no result/query cache can serve the measurement."""

import json
import statistics
import sys
import time
import urllib.request

QUERY = "avoid retrying the same position twice"


def one(base, index, nonce=""):
    body = json.dumps({
        "query": {"semantic": {"field": "body", "query": QUERY + nonce, "k": 10}},
        "size": 10, "_source": ["title"],
    }).encode()
    req = urllib.request.Request(f"{base}/{index}/_search", data=body,
                                 headers={"Content-Type": "application/json"})
    t0 = time.time()
    with urllib.request.urlopen(req, timeout=300) as r:
        r.read()
    return (time.time() - t0) * 1000


def main():
    base, index, label = sys.argv[1], sys.argv[2], sys.argv[3]
    one(base, index, " warmup")  # warm the model + page cache
    samples = [one(base, index, f" variant {i}") for i in range(5)]
    print(f"{label}: median={statistics.median(samples):.1f}ms  "
          f"min={min(samples):.1f}ms max={max(samples):.1f}ms  "
          f"samples={[round(s, 1) for s in samples]}")


if __name__ == "__main__":
    main()
