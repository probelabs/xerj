#!/usr/bin/env python3
import json
import pathlib
import statistics

root = pathlib.Path(__file__).resolve().parent
summary = {}
for label in ("base", "head"):
    rows = [
        json.loads(line)
        for line in (root / f"{label}.jsonl").read_text(encoding="utf-8").splitlines()
    ]
    summary[label] = {
        key: statistics.median(row[key] for row in rows)
        for key in (
            "get_rps",
            "get_us_p50",
            "get_us_p95",
            "get_us_p99",
            "refresh_ms_p50",
            "refresh_ms_p95",
        )
    }
summary["difference_pct"] = {
    key: (summary["head"][key] / summary["base"][key] - 1.0) * 100.0
    for key in summary["base"]
}
(root / "summary.json").write_text(
    json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="utf-8"
)
print(json.dumps(summary, indent=2, sort_keys=True))
