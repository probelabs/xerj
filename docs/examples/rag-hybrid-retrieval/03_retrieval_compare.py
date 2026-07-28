#!/usr/bin/env python3
"""RAG retrieval: pure cosine (pgvector-style) vs BM25 vs RRF hybrid, over the
doc chunks. Shows where dense-only retrieval buries exact identifiers, and how
hybrid recovers them without losing concept-matching. Needs XERJ :9200 + Ollama."""
import json, urllib.request
X="http://127.0.0.1:9200"; OLL="http://127.0.0.1:11434/api/embeddings"
def embed(t):
    r=urllib.request.Request(OLL,data=json.dumps({"model":"embeddinggemma","prompt":t}).encode(),headers={"Content-Type":"application/json"})
    return json.loads(urllib.request.urlopen(r).read())["embedding"]
def S(b):
    r=urllib.request.Request(f"{X}/docchunks/_search",data=json.dumps(b).encode(),headers={"Content-Type":"application/json"})
    return json.loads(urllib.request.urlopen(r).read())
def rank(sub,b):
    for i,h in enumerate(S(b)["hits"]["hits"]):
        if sub in h["_source"]["heading"]: return i+1
    return ">10"
def cosine(qv): return {"query":{"knn":{"field":"vec","query_vector":qv,"k":18,"num_candidates":18}},"size":18}
def bm25(q):    return {"query":{"match":{"text":q}},"size":18}
def hybrid(q,qv): return {"query":{"hybrid":{"queries":[
    {"query":{"match":{"text":q}},"weight":1.0},
    {"query":{"knn":{"field":"vec","query_vector":qv,"num_candidates":18}},"weight":1.0}],
    "fusion":{"type":"rrf","k":60}}},"size":18}

tests={"E4304":"Error E4304","createCheckoutSession":"createCheckoutSession",
       "WEBHOOK_SIGNING_SECRET":"Verifying webhook","v2.11":"Migrating to API v2.11"}
print(f"  {'exact-term query':26} {'cosine':>7} {'BM25':>6} {'hybrid':>7}   (rank of the correct chunk)")
for q,correct in tests.items():
    qv=embed(q)
    print(f"  {q:26} {str(rank(correct,cosine(qv))):>7} {str(rank(correct,bm25(q))):>6} {str(rank(correct,hybrid(q,qv))):>7}")
print("\n  <=k means the chunk is inside a RAG top-k window; cosine's #13/#14 are invisible to the LLM.")
