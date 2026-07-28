import json, urllib.request
X="http://127.0.0.1:9200"; OLL="http://127.0.0.1:11434/api/embeddings"
def embed(t):
    r=urllib.request.Request(OLL,data=json.dumps({"model":"embeddinggemma","prompt":t}).encode(),headers={"Content-Type":"application/json"})
    return json.loads(urllib.request.urlopen(r).read())["embedding"]
def x(path,body=None,method="POST",nd=False):
    ct="application/x-ndjson" if nd else "application/json"
    data=body.encode() if isinstance(body,str) else (json.dumps(body).encode() if body is not None else None)
    r=urllib.request.Request(f"{X}/{path}",data=data,headers={"Content-Type":ct},method=method)
    try: return json.loads(urllib.request.urlopen(r).read())
    except Exception as e: return {"error":str(e)[:150]}
docs=json.load(open("/tmp/claude-1001/-home-claude-ai-xerj/09b4eaff-87bf-4097-b72f-3907d1ef6cd4/scratchpad/rag/chunks.json"))
x("docchunks",method="DELETE")
x("docchunks",{"mappings":{"properties":{
   "doc":{"type":"keyword"},"heading":{"type":"text"},"text":{"type":"text"},"body":{"type":"text"},
   "vec":{"type":"dense_vector","dims":768,"similarity":"cosine"}}}},"PUT")
lines=[]
for d in docs:
    dd=dict(d); dd["vec"]=embed(d["text"])
    lines.append(json.dumps({"index":{"_id":d["id"]}})); lines.append(json.dumps(dd))
print("indexed:", x("docchunks/_bulk","\n".join(lines)+"\n",nd=True).get("errors"))
x("docchunks/_refresh")
