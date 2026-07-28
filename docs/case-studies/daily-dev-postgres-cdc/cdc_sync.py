#!/usr/bin/env python3
"""Postgres -> XERJ CDC sync via logical replication (test_decoding slot).
The slot streams every INSERT/UPDATE/DELETE on `post`; for each changed id we
read the current row, embed title+summary, and upsert into XERJ. Deletes remove
the doc. Keeps XERJ in sync automatically — no manual reindex."""
import json, re, sys, time, urllib.request
import psycopg2
X="http://127.0.0.1:9200"; OLL="http://127.0.0.1:11434/api/embeddings"
PG=dict(host="127.0.0.1",port=5433,dbname="dailydev",user="postgres",password="pw")
def embed(t):
    r=urllib.request.Request(OLL,data=json.dumps({"model":"embeddinggemma","prompt":t}).encode(),headers={"Content-Type":"application/json"})
    return json.loads(urllib.request.urlopen(r).read())["embedding"]
def xreq(path,body=None,method="POST",ndjson=False):
    ct="application/x-ndjson" if ndjson else "application/json"
    data=body.encode() if isinstance(body,str) else (json.dumps(body).encode() if body is not None else None)
    r=urllib.request.Request(f"{X}/{path}",data=data,headers={"Content-Type":ct},method=method)
    try: return json.loads(urllib.request.urlopen(r).read())
    except Exception as e: return {"error":str(e)[:200]}

def ensure_index():
    xreq("posts",method="DELETE")
    xreq("posts",{"mappings":{"properties":{
        "id":{"type":"keyword"},"title":{"type":"text"},"summary":{"type":"text"},
        "tags":{"type":"keyword"},"source_id":{"type":"keyword"},
        "views":{"type":"integer"},"upvotes":{"type":"integer"},"comments":{"type":"integer"},
        "score":{"type":"integer"},"trending":{"type":"integer"},
        "created_at":{"type":"date"},
        "vec":{"type":"dense_vector","dims":768,"similarity":"cosine"}}}},method="PUT")

# parse test_decoding lines: "table public.post: INSERT: id[text]:'p1' ..."
CHG=re.compile(r"table public\.post: (INSERT|UPDATE|DELETE): .*?\bid\[text\]:'([^']+)'")
def drain_slot(cur):
    cur.execute("SELECT data FROM pg_logical_slot_get_changes('xerj_slot',NULL,NULL);")
    ops={}   # id -> last op
    for (data,) in cur.fetchall():
        m=CHG.search(data)
        if m: ops[m.group(2)]=m.group(1)
    return ops

def sync(ids_ops, cur):
    up=[]; delete=[]
    for pid,op in ids_ops.items():
        if op=="DELETE": delete.append(pid); continue
        cur.execute("SELECT id,title,summary,tags_str,source_id,views,upvotes,comments,score,trending,visible,deleted,created_at FROM post WHERE id=%s",(pid,))
        row=cur.fetchone()
        if not row or row[11] or not row[10]:   # deleted or not visible -> remove
            delete.append(pid); continue
        d=dict(id=row[0],title=row[1],summary=row[2],tags=(row[3] or "").split(","),
                source_id=row[4],views=row[5],upvotes=row[6],comments=row[7],score=row[8],
                trending=row[9],created_at=row[12].isoformat())
        d["vec"]=embed(f"{row[1]}. {row[2]}")
        up.append(d)
    lines=[]
    for d in up:
        lines.append(json.dumps({"index":{"_id":d["id"]}})); lines.append(json.dumps(d))
    for pid in delete:
        lines.append(json.dumps({"delete":{"_id":pid}}))
    if lines:
        r=xreq("posts/_bulk","\n".join(lines)+"\n",ndjson=True)
        xreq("posts/_refresh",method="POST")
        return len(up),len(delete),r.get("errors")
    return 0,0,False

if __name__=="__main__":
    conn=psycopg2.connect(**PG); conn.autocommit=True; cur=conn.cursor()
    if "--init" in sys.argv: ensure_index()
    loops = int(sys.argv[sys.argv.index("--loops")+1]) if "--loops" in sys.argv else 1
    for i in range(loops):
        ops=drain_slot(cur)
        if ops:
            u,dd,err=sync(ops,cur)
            print(f"[cdc] synced {u} upserts, {dd} deletes to XERJ (errors={err})  changed ids: {list(ops.items())}")
        else:
            print("[cdc] no changes")
        if loops>1: time.sleep(2)
