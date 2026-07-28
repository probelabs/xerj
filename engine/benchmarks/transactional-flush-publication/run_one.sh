#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 4 ]; then
  echo "usage: $0 <label> <binary> <data-dir> <output-jsonl>" >&2
  exit 2
fi

bench_label=$1
bench_binary=$2
bench_data_dir=$3
bench_output=$4

mkdir -p "$bench_data_dir"
rm -f "$bench_output"

RUST_LOG=error "$bench_binary" --insecure --data-dir "$bench_data_dir" >"${bench_output}.server.log" 2>&1 &
bench_pid=$!
cleanup() {
  kill -INT "$bench_pid" 2>/dev/null || true
  wait "$bench_pid" 2>/dev/null || true
}
trap cleanup EXIT

python3 - "$bench_label" "$bench_output" <<'PY'
import http.client
import json
import os
import statistics
import sys
import time

label, output_path = sys.argv[1:]

for _ in range(200):
    try:
        probe = http.client.HTTPConnection("127.0.0.1", 9200, timeout=1)
        probe.request("GET", "/")
        response = probe.getresponse()
        response.read()
        if response.status < 500:
            break
    except OSError:
        time.sleep(0.05)
else:
    raise RuntimeError("server did not become ready")

connection = http.client.HTTPConnection("127.0.0.1", 9200, timeout=10)

def request(method, path, body=None, headers=None):
    connection.request(method, path, body=body, headers=headers or {})
    response = connection.getresponse()
    payload = response.read()
    if response.status >= 300:
        raise RuntimeError((method, path, response.status, payload[:500]))
    return payload

def percentile(values, fraction):
    ordered = sorted(values)
    return ordered[int((len(ordered) - 1) * fraction)]

request(
    "PUT",
    "/bench",
    json.dumps(
        {
            "mappings": {
                "properties": {
                    "body": {"type": "text"},
                    "n": {"type": "integer"},
                }
            }
        }
    ),
    {"Content-Type": "application/json"},
)
bulk_body = "".join(
    json.dumps({"index": {"_id": str(index)}})
    + "\n"
    + json.dumps({"body": f"document {index}", "n": index})
    + "\n"
    for index in range(1000)
)
request(
    "POST",
    "/bench/_bulk?refresh=true",
    bulk_body,
    {"Content-Type": "application/x-ndjson"},
)

count = json.loads(request("GET", "/bench/_count"))
if count["count"] != 1000:
    raise AssertionError(("initial count", count))
for index in range(1000):
    document = json.loads(request("GET", f"/bench/_doc/{index}"))
    if not document.get("found") or document["_source"]["n"] != index:
        raise AssertionError(("warmup correctness", index, document))

with open(output_path, "w", encoding="utf-8") as output:
    for trial in range(5):
        timings_us = []
        started = time.perf_counter()
        for index in range(20000):
            expected = index % 1000
            before = time.perf_counter_ns()
            document = json.loads(request("GET", f"/bench/_doc/{expected}"))
            timings_us.append((time.perf_counter_ns() - before) / 1000)
            if not document.get("found") or document["_source"]["n"] != expected:
                raise AssertionError(("measured GET correctness", expected, document))
        elapsed = time.perf_counter() - started

        refresh_ms = []
        for index in range(30):
            document_id = f"{label}-{trial}-{index}"
            request(
                "PUT",
                f"/bench/_doc/{document_id}",
                json.dumps({"body": "refresh probe", "n": index}),
                {"Content-Type": "application/json"},
            )
            before = time.perf_counter_ns()
            request("POST", "/bench/_refresh")
            refresh_ms.append((time.perf_counter_ns() - before) / 1_000_000)
            document = json.loads(request("GET", f"/bench/_doc/{document_id}"))
            if not document.get("found") or document["_source"]["n"] != index:
                raise AssertionError(("refresh correctness", document_id, document))

        row = {
            "label": label,
            "trial": trial,
            "get_requests": 20000,
            "get_rps": 20000 / elapsed,
            "get_us_p50": percentile(timings_us, 0.50),
            "get_us_p95": percentile(timings_us, 0.95),
            "get_us_p99": percentile(timings_us, 0.99),
            "refreshes": 30,
            "refresh_ms_p50": percentile(refresh_ms, 0.50),
            "refresh_ms_p95": percentile(refresh_ms, 0.95),
            "correctness": "passed",
        }
        output.write(json.dumps(row, sort_keys=True) + "\n")
        output.flush()
        print(json.dumps(row, sort_keys=True))

final_count = json.loads(request("GET", "/bench/_count"))["count"]
expected_count = 1000 + 5 * 30
if final_count != expected_count:
    raise AssertionError(("final count", final_count, expected_count))
PY
