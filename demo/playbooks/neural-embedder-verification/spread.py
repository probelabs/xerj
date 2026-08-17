"""Score-spread diagnostic: top-hit / 10th-hit ratio on a semantic query,
plus the rank of the ground-truth file. Same shape for lexical and neural."""

import json
import sys
import urllib.request

QUERIES = [
    ("bounded backtracking visited set", "backtrack"),
    ("avoid retrying the same position twice", "backtrack"),
]


def search(base, index, field, query, k=10):
    body = json.dumps(
        {
            "query": {"semantic": {"field": field, "query": query, "k": k}},
            "size": k,
            "_source": ["title", "ax_path", "path"],
        }
    ).encode()
    req = urllib.request.Request(
        f"{base}/{index}/_search",
        data=body,
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=300) as r:
        return json.load(r)


def report(base, index, field, label, queries):
    print(f"\n=== {label}  ({base} / {index}) ===")
    for q, expect in queries:
        try:
            res = search(base, index, field, q)
        except Exception as e:
            print(f"  {q!r}: ERROR {e}")
            continue
        hits = res["hits"]["hits"]
        if not hits:
            print(f"  {q!r}: no hits")
            continue
        scores = [h["_score"] for h in hits]
        top = scores[0]
        tenth = scores[min(9, len(scores) - 1)]
        spread = top / tenth if tenth else float("inf")
        names = [
            h["_source"].get("title")
            or h["_source"].get("ax_path")
            or h["_source"].get("path")
            for h in hits
        ]
        rank = next((i + 1 for i, n in enumerate(names) if n and expect in n), None)
        print(f"  query: {q!r}")
        print(
            f"    expect~{expect}  rank={rank}  spread(top/10th)={spread:.4f}  "
            f"top={top:.5f} 10th={tenth:.5f}  n={len(hits)}"
        )
        print(f"    top5: {names[:5]}")


if __name__ == "__main__":
    base, index, field, label = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
    report(base, index, field, label, QUERIES)
