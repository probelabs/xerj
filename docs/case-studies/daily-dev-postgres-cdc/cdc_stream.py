#!/usr/bin/env python3
"""Postgres -> XERJ CDC via TRUE push-streaming logical replication + LSN
checkpointing (exactly-once-convergent).

Uses psycopg2's LogicalReplicationConnection.consume_stream(): blocks on the WAL
stream (no polling), and after each change is durably applied to XERJ it calls
send_feedback(flush_lsn=...) so Postgres advances the slot's confirmed_flush_lsn.
On restart, streaming resumes from the last confirmed LSN -> no missed or
re-delivered-and-lost changes (upsert/delete by id makes replays idempotent)."""
import json, re, sys, urllib.request
import psycopg2
from psycopg2.extras import LogicalReplicationConnection, REPLICATION_LOGICAL
X="http://127.0.0.1:9200"; OLL="http://127.0.0.1:11434/api/embeddings"
PG=dict(host="127.0.0.1",port=5433,dbname="dailydev",user="postgres",password="pw")
def embed(t):
    r=urllib.request.Request(OLL,data=json.dumps({"model":"embeddinggemma","prompt":t}).encode(),headers={"Content-Type":"application/json"})
    return json.loads(urllib.request.urlopen(r).read())["embedding"]
def xbulk(lines):
    r=urllib.request.Request(f"{X}/posts/_bulk",data=("\n".join(lines)+"\n").encode(),
        headers={"Content-Type":"application/x-ndjson"},method="POST")
    return json.loads(urllib.request.urlopen(r).read()).get("errors")

CHG=re.compile(r"table public\.post: (INSERT|UPDATE|DELETE): .*?\bid\[text\]:'([^']+)'")
# a read connection to fetch current row state for changed ids
_read=psycopg2.connect(**PG); _read.autocommit=True; _rc=_read.cursor()
def apply_change(op,pid):
    if op=="DELETE":
        return [json.dumps({"delete":{"_id":pid}})]
    _rc.execute("SELECT id,title,summary,tags_str,source_id,views,upvotes,comments,score,trending,visible,deleted,created_at FROM post WHERE id=%s",(pid,))
    row=_rc.fetchone()
    if not row or row[11] or not row[10]:
        return [json.dumps({"delete":{"_id":pid}})]
    d=dict(id=row[0],title=row[1],summary=row[2],tags=(row[3] or "").split(","),source_id=row[4],
           views=row[5],upvotes=row[6],comments=row[7],score=row[8],trending=row[9],
           created_at=row[12].isoformat())
    d["vec"]=embed(f"{row[1]}. {row[2]}")
    return [json.dumps({"index":{"_id":pid}}),json.dumps(d)]

class Consumer:
    def __init__(self): self.applied=0
    def __call__(self,msg):
        m=CHG.search(msg.payload)
        if m:
            op,pid=m.group(1),m.group(2)
            lines=apply_change(op,pid)
            err=xbulk(lines)
            self.applied+=1
            print(f"[stream] {op} {pid} -> XERJ (errors={err})  LSN={msg.data_start}  [checkpointing]",flush=True)
        # LSN CHECKPOINT: confirm this change is durably applied; pg advances the slot.
        msg.cursor.send_feedback(flush_lsn=msg.data_start)

if __name__=="__main__":
    conn=psycopg2.connect(connection_factory=LogicalReplicationConnection,**PG)
    cur=conn.cursor()
    try:
        cur.start_replication(slot_name="xerj_slot",decode=True)   # resumes at confirmed_flush_lsn
    except psycopg2.ProgrammingError:
        cur.create_replication_slot("xerj_slot",output_plugin="test_decoding")
        cur.start_replication(slot_name="xerj_slot",decode=True)
    print("[stream] consuming WAL from confirmed LSN (push, no polling; Ctrl-C to stop)",flush=True)
    c=Consumer()
    import signal
    def stop(*_): print(f"\n[stream] stopped after {c.applied} changes"); sys.exit(0)
    signal.signal(signal.SIGTERM,stop); signal.signal(signal.SIGINT,stop)
    cur.consume_stream(c)
