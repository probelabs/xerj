"""Paraphrase-retrieval probe: queries deliberately worded WITHOUT the target
file's identifying vocabulary. Reports the rank of the ground-truth file and
the top-hit / 10th-hit score spread, for whichever backend the server runs."""

import json
import sys
import urllib.request

# (query, expected filename substring). Each query avoids the target's own
# distinctive tokens so lexical overlap cannot carry the retrieval.
PROBES = [
    ("bounded backtracking visited set", "backtrack.rs"),
    ("avoid retrying the same position twice", "backtrack.rs"),
    ("build the state machine lazily while searching instead of ahead of time",
     "hybrid"),
    ("reuse scratch allocations across calls instead of allocating each time",
     "pool.rs"),
    ("a machine that never has to try an alternative path", "onepass.rs"),
]


def search(base, index, field, query, k=10):
    body = json.dumps({
        "query": {"semantic": {"field": field, "query": query, "k": k}},
        "size": k,
        "_source": ["title", "path"],
    }).encode()
    req = urllib.request.Request(f"{base}/{index}/_search", data=body,
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=300) as r:
        return json.load(r)


def main():
    base, index, field, label = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
    print(f"\n=== {label} ===")
    found, spreads = 0, []
    for query, expect in PROBES:
        res = search(base, index, field, query)
        hits = res["hits"]["hits"]
        paths = [h["_source"].get("path", "") for h in hits]
        titles = [h["_source"].get("title", "") for h in hits]
        rank = next((i + 1 for i, (p, t) in enumerate(zip(paths, titles))
                     if expect in p or expect in t), None)
        scores = [h["_score"] for h in hits]
        spread = scores[0] / scores[min(9, len(scores) - 1)]
        spreads.append(spread)
        if rank:
            found += 1
        print(f"  {query!r}")
        print(f"    expect~{expect}  rank={rank}  spread={spread:.4f}  "
              f"top1={titles[0]}")
    print(f"  --> found in top-10: {found}/{len(PROBES)}   "
          f"mean spread={sum(spreads) / len(spreads):.4f}")


if __name__ == "__main__":
    main()
