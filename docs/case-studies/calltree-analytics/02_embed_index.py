import json, urllib.request, time
X="http://127.0.0.1:9200"; OLL="http://127.0.0.1:11434/api/embeddings"
d=json.load(open("/tmp/claude-1001/-home-claude-ai-xerj/09b4eaff-87bf-4097-b72f-3907d1ef6cd4/scratchpad/rob/dataset.json"))
def embed(text):
    req=urllib.request.Request(OLL,data=json.dumps({"model":"embeddinggemma","prompt":text}).encode(),
        headers={"Content-Type":"application/json"})
    return json.loads(urllib.request.urlopen(req).read())["embedding"]
DIMS=len(embed("probe")); print("embedding dims:",DIMS)

def put(path,body,method="PUT"):
    r=urllib.request.Request(f"{X}/{path}",data=json.dumps(body).encode() if body else None,
        headers={"Content-Type":"application/json"},method=method)
    try: return urllib.request.urlopen(r).read()
    except Exception as e: return str(e).encode()

# --- conversations index: structured columns + dense_vector for retrieval ---
put("conversations",None,"DELETE")
put("conversations",{"mappings":{"properties":{
  "band":{"type":"keyword"},"issue":{"type":"keyword"},"resolution":{"type":"keyword"},
  "device":{"type":"keyword"},"agent":{"type":"keyword"},"channel":{"type":"keyword"},
  "csat":{"type":"integer"},"handle_time_s":{"type":"integer"},"day":{"type":"integer"},
  "body":{"type":"text"},
  "vec":{"type":"dense_vector","dims":DIMS,"similarity":"cosine"}}}})
# --- KB index (canonical records) ---
put("kb",None,"DELETE")
put("kb",{"mappings":{"properties":{"kb_id":{"type":"keyword"},"title":{"type":"text"},
  "body":{"type":"text"},"vec":{"type":"dense_vector","dims":DIMS,"similarity":"cosine"}}}})

def bulk(index, rows, textfn):
    lines=[]
    for i,r in enumerate(rows):
        r=dict(r); r["vec"]=embed(textfn(r))
        lines.append(json.dumps({"index":{"_id":str(i)}})); lines.append(json.dumps(r))
    body=("\n".join(lines)+"\n").encode()
    rq=urllib.request.Request(f"{X}/{index}/_bulk",data=body,
        headers={"Content-Type":"application/x-ndjson"},method="POST")
    return json.loads(urllib.request.urlopen(rq).read()).get("errors")
t=time.time()
e1=bulk("conversations", d["docs"], lambda r: r["body"])
e2=bulk("kb", d["kb"], lambda r: r["title"]+". "+r["body"])
put("conversations/_refresh",None,"POST"); put("kb/_refresh",None,"POST")
print(f"indexed {len(d['docs'])} conversations (errors={e1}) + {len(d['kb'])} KB (errors={e2}) in {time.time()-t:.1f}s")
