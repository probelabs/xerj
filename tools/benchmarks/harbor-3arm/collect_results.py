#!/usr/bin/env python3
"""Collect the XERJ 3-arm campaign into one aligned report.

Walks harbor-campaign/jobs/<bench>.<model>.<arm>/ job dirs, reads each trial's
result (reward + agent metrics), and prints:
  * per-cell resolve rate with Wilson 95% CI
  * COMPLETE-MATRIX cross sections (only tasks finished in EVERY arm of a
    benchmark+model count toward arm-vs-arm claims — same rule that caught the
    unaligned 2026-08-18 run)
  * cost, tokens, wall-clock per cell and per solve

Trial layout (harbor): <job>/<task>__<trial-id>/result.json  (fields observed
from harbor 0.21; unknown fields are tolerated, missing trials reported).
"""
import json
import math
import sys
from collections import defaultdict
from pathlib import Path

JOBS = Path(__file__).parent / "jobs"


def wilson(k, n, z=1.96):
    if n == 0:
        return (0.0, 0.0)
    p = k / n
    d = 1 + z * z / n
    c = p + z * z / (2 * n)
    m = z * math.sqrt(p * (1 - p) / n + z * z / (4 * n * n))
    return (max(0.0, (c - m) / d), min(1.0, (c + m) / d))


def load_trials(job_dir: Path):
    """Yield (task, dict) per finished trial; tolerant of layout drift."""
    for rj in sorted(job_dir.glob("**/result.json")):
        try:
            r = json.loads(rj.read_text())
        except (json.JSONDecodeError, OSError):
            continue
        task = (r.get("task_name") or r.get("task", {}).get("name")
                or rj.parent.name.split("__")[0])
        reward = None
        for path in (("verifier_result", "reward"), ("reward",), ("result", "reward")):
            v = r
            for k in path:
                v = v.get(k) if isinstance(v, dict) else None
            if v is not None:
                reward = v
                break
        if isinstance(reward, dict):        # named rewards -> main one
            reward = reward.get("reward", reward.get("resolved"))
        am = r.get("agent_result") or {}
        yield task, {
            "reward": reward,
            "resolved": (reward or 0) >= 1 if reward is not None else None,
            "cost": am.get("total_cost_usd") or r.get("total_cost_usd"),
            "in_tok": am.get("input_tokens"),
            "out_tok": am.get("output_tokens"),
            "sec": r.get("elapsed_seconds") or am.get("elapsed_seconds"),
            "exception": r.get("exception_info") or r.get("error"),
        }


def main():
    cells = defaultdict(dict)               # (bench, model, arm) -> {task: trial}
    for job_dir in sorted(JOBS.iterdir()) if JOBS.exists() else []:
        if not job_dir.is_dir():
            continue
        parts = job_dir.name.split(".")
        if len(parts) < 3:
            continue
        bench, model, arm = parts[0], ".".join(parts[1:-1]), parts[-1]
        for task, t in load_trials(job_dir):
            cells[(bench, model, arm)][task] = t

    if not cells:
        sys.exit(f"no trials under {JOBS} — run run_matrix.sh first")

    print("=" * 90)
    print("XERJ 3-ARM CAMPAIGN — per-cell status")
    print("=" * 90)
    fmt = "{:<20} {:<16} {:<11} {:>5} {:>7} {:>16} {:>9} {:>9}"
    print(fmt.format("benchmark", "model", "arm", "n", "solved", "95% CI",
                     "$ total", "hrs"))
    for (bench, model, arm), tasks in sorted(cells.items()):
        done = {t: v for t, v in tasks.items() if v["reward"] is not None}
        k = sum(1 for v in done.values() if v["resolved"])
        lo, hi = wilson(k, len(done))
        cost = sum(v["cost"] or 0 for v in done.values())
        hrs = sum(v["sec"] or 0 for v in done.values()) / 3600
        errs = sum(1 for v in tasks.values() if v["exception"])
        note = f"  !! {errs} errored" if errs else ""
        print(fmt.format(bench, model, arm, len(done), k,
                         f"[{lo:.0%},{hi:.0%}]", f"{cost:.2f}", f"{hrs:.1f}")
              + note)

    print()
    print("=" * 90)
    print("COMPLETE MATRIX — arm comparison on identical tasks (per benchmark+model)")
    print("=" * 90)
    by_bm = defaultdict(dict)
    for (bench, model, arm), tasks in cells.items():
        by_bm[(bench, model)][arm] = {
            t for t, v in tasks.items() if v["reward"] is not None}
    for (bench, model), arms in sorted(by_bm.items()):
        if len(arms) < 2:
            continue
        common = set.intersection(*arms.values())
        print(f"\n{bench} / {model}: {len(common)} tasks finished in all "
              f"{len(arms)} arms")
        for arm in sorted(arms):
            tv = cells[(bench, model, arm)]
            k = sum(1 for t in common if tv[t]["resolved"])
            rew = [tv[t]["reward"] for t in common
                   if isinstance(tv[t]["reward"], (int, float))]
            mean_r = sum(rew) / len(rew) if rew else float("nan")
            cost = sum(tv[t]["cost"] or 0 for t in common)
            sec = sum(tv[t]["sec"] or 0 for t in common)
            print(f"   {arm:<11} resolved {k}/{len(common)}   "
                  f"mean-reward {mean_r:.3f}   ${cost:.2f}   {sec/3600:.1f}h")


if __name__ == "__main__":
    main()
