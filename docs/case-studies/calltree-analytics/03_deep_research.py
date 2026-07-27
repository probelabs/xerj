#!/usr/bin/env python3
"""calltree.ai deep-research task, ONE request each — using the rc.6
knn+aggregations feature. Needs XERJ :9200 + Ollama (embeddinggemma + an LLM)."""
import json, urllib.request
X="http://127.0.0.1:9200"; OLL="http://127.0.0.1:11434"
def embed(t):
    r=urllib.request.Request(f"{OLL}/api/embeddings",data=json.dumps({"model":"embeddinggemma","prompt":t}).encode(),headers={"Content-Type":"application/json"})
    return json.loads(urllib.request.urlopen(r).read())["embedding"]
def S(b,idx="conversations"):
    r=urllib.request.Request(f"{X}/{idx}/_search",data=json.dumps(b).encode(),headers={"Content-Type":"application/json"})
    return json.loads(urllib.request.urlopen(r).read())
def gen(prompt):
    r=urllib.request.Request(f"{OLL}/api/generate",data=json.dumps({"model":"qwen2.5:7b","prompt":prompt,"stream":False}).encode(),headers={"Content-Type":"application/json"})
    return json.loads(urllib.request.urlopen(r).read())["response"].strip()

qv=embed("wifi connectivity problems dropouts slow weak signal range")
print('"of calls about wifi issues, how many 2.4 vs 5GHz, and what are the key issues?"\n')
# ONE request: semantic retrieval + band split + key issues + csat + example hits
r=S({"query":{"knn":{"field":"vec","query_vector":qv,"k":80,"num_candidates":130}},
     "size":3,"_source":["band","issue","body"],
     "aggs":{"by_band":{"terms":{"field":"band"},"aggs":{
        "key_issues":{"terms":{"field":"issue","size":4}},
        "csat":{"percentiles":{"field":"csat","percents":[50]}},
        "aht":{"avg":{"field":"handle_time_s"}}}}}})
buckets=[b for b in r["aggregations"]["by_band"]["buckets"] if b["key"]!="n/a"]
tot=sum(b["doc_count"] for b in buckets)
print(f"retrieved {r['hits']['total']['value']} nearest wifi calls; aggregated in the same request:\n")
for b in buckets:
    iss=", ".join(x["key"] for x in b["key_issues"]["buckets"])
    print(f"  {b['key']:7} {b['doc_count']:2} ({100*b['doc_count']/tot:.0f}%)  csat_p50={b['csat']['values']['50.0']}  aht={b['aht']['value']:.0f}s  issues: {iss}")
# LLM synthesis over the example hits returned in the SAME response
ex="\n".join(f"- [{h['_source']['band']}/{h['_source']['issue']}] {h['_source']['body']}" for h in r["hits"]["hits"])
print("\ndeep-research narrative (LLM over the retrieved examples):")
print(" ", gen(f"Support calls about wifi:\n{ex}\n\nOne-sentence executive summary of the key issue.")[:300])
