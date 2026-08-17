"""Minimal semantic sanity check: three short sentences, one paraphrase pair
with almost no shared vocabulary, one unrelated. A backend that models meaning
must rank the paraphrase above the unrelated sentence."""

import json
import sys
import time
import urllib.error
import urllib.request

DOCS = [
    ("guitar", "A man is playing a guitar on stage."),
    ("musician", "A performer strums six strings in front of a crowd."),
    ("finance", "The quarterly financial report shows rising interest rates."),
]
QUERY = "someone entertaining an audience with a stringed instrument"


def req(method, url, payload=None):
    data = payload.encode() if isinstance(payload, str) else payload
    r = urllib.request.Request(url, data=data, method=method,
                               headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(r, timeout=600) as resp:
            return resp.status, resp.read().decode()
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode()


def main():
    base, index = sys.argv[1], sys.argv[2]
    req("DELETE", f"{base}/{index}")
    status, body = req("PUT", f"{base}/{index}", json.dumps(
        {"mappings": {"properties": {"title": {"type": "keyword"},
                                     "body": {"type": "semantic_text"}}}}))
    print(f"PUT {index}: {status} {body[:200]}")
    t0 = time.time()
    for title, text in DOCS:
        status, body = req("POST", f"{base}/{index}/_doc?refresh=true",
                           json.dumps({"title": title, "body": text}))
        if status >= 300:
            print(f"index {title} FAILED {status}: {body[:400]}")
            sys.exit(1)
    print(f"indexed {len(DOCS)} docs in {time.time() - t0:.1f}s")

    status, body = req("POST", f"{base}/{index}/_search", json.dumps(
        {"query": {"semantic": {"field": "body", "query": QUERY, "k": 3}},
         "size": 3, "_source": ["title"]}))
    hits = json.loads(body)["hits"]["hits"]
    print(f"query: {QUERY!r}")
    for h in hits:
        print(f"   {h['_source']['title']:10s} score={h['_score']:.5f}")
    names = [h["_source"]["title"] for h in hits]
    top = hits[0]["_score"]
    bottom = hits[-1]["_score"]
    print(f"   ranking={names}  top/bottom spread={top / bottom:.4f}")


if __name__ == "__main__":
    main()
