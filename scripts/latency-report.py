#!/usr/bin/env python3
"""Summarize opt-in Fig latency logs by (session, revision), never by line order.
DOM/RAF are not pixel presentation. Native/JS clock correlation has ~1ms precision.
Usage: python3 scripts/latency-report.py LOG --output DIRECTORY
"""

from pathlib import Path
import argparse, json, math, collections, statistics, re

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("log", type=Path)
parser.add_argument("--output", required=True, type=Path)
args = parser.parse_args()
out = args.output
out.mkdir(parents=True, exist_ok=True)
samples = []
for line in args.log.read_text().splitlines():
    line = re.sub(r"\x1b\[[0-9;]*m", "", line)
    for marker, batch in [("webview samples=", True), ("native sample=", False)]:
        if marker in line and "fig_latency" in line:
            try:
                value = json.loads(line.split(marker, 1)[1])
                samples.extend(value if batch else [value])
            except (ValueError, TypeError):
                raise SystemExit("Malformed latency record; capture may be incomplete")
            break
(out / "samples.json").write_text(json.dumps(samples, indent=2))
groups = collections.defaultdict(lambda: collections.defaultdict(list))
for s in samples:
    if s.get("sessionId") and s.get("revision") is not None:
        groups[(s["sessionId"], s["revision"])][s["stage"]].append(s)


def stats(xs):
    xs = sorted(xs)
    return (
        {
            "n": len(xs),
            "minMs": xs[0],
            "p50Ms": statistics.median(xs),
            "p95Ms": xs[max(0, math.ceil(len(xs) * 0.95) - 1)],
            "maxMs": xs[-1],
        }
        if xs
        else None
    )


aggregate = {
    stage: stats(
        [
            s["milliseconds"]
            for s in samples
            if s["stage"] == stage and s.get("milliseconds") is not None
        ]
    )
    for stage in sorted({s["stage"] for s in samples})
}
rows = []
for (session, revision), g in groups.items():
    if "hook-received" not in g:
        continue
    first = lambda stage: g.get(stage, [None])[0]
    hook = first("hook-received")
    r = {"session": session, "revision": revision, "hookUnixMs": hook["unixMs"]}
    stages = [
        "main-queue",
        "webview-dispatch",
        "input-received",
        "input-to-current-suggestions-dom",
        "input-to-settled-dom-commit",
        "current-suggestions-next-frame",
    ]
    for stage in stages:
        value = first(stage)
        r[stage + "Ms"] = value["unixMs"] - hook["unixMs"] if value else None
    if first("main-queue"):
        r["queueMonotonicMs"] = first("main-queue")["milliseconds"]
    for stage in ["parse", "ranking", "process", "spec-fetch", "caret-update"]:
        r[stage + "TotalMs"] = sum(x["milliseconds"] for x in g[stage])
        r[stage + "Count"] = len(g[stage])
    if first("input-to-current-suggestions-dom"):
        r["webviewToCurrentDomMs"] = first("input-to-current-suggestions-dom")[
            "milliseconds"
        ]
        r["suggestionCount"] = first("input-to-current-suggestions-dom")[
            "suggestionCount"
        ]
    r["settledVisible"] = bool(
        first("input-to-settled-dom-commit")
        and first("input-to-settled-dom-commit").get("visible")
        and first("input-to-settled-dom-commit").get("suggestionCount", 0) > 0
    )
    rows.append(r)
rows.sort(key=lambda r: r["hookUnixMs"])
for i, r in enumerate(rows):
    r["previousInputGapMs"] = (
        r["hookUnixMs"] - rows[i - 1]["hookUnixMs"]
        if i and rows[i - 1]["session"] == r["session"]
        else None
    )
metrics = [
    "main-queueMs",
    "webview-dispatchMs",
    "input-receivedMs",
    "input-to-current-suggestions-domMs",
    "current-suggestions-next-frameMs",
    "webviewToCurrentDomMs",
    "queueMonotonicMs",
]
summary = {k: stats([r[k] for r in rows if r.get(k) is not None]) for k in metrics}
summary["hook-to-settled-visible-domMs"] = stats(
    [r["input-to-settled-dom-commitMs"] for r in rows if r["settledVisible"]]
)
subset = {
    label: {
        k: stats([r[k] for r in rows if pred(r) and r.get(k) is not None])
        for k in metrics
    }
    for label, pred in [
        (
            "under100msGap",
            lambda r: (
                r["previousInputGapMs"] is not None and r["previousInputGapMs"] < 100
            ),
        ),
        (
            "over500msGap",
            lambda r: (
                r["previousInputGapMs"] is not None and r["previousInputGapMs"] > 500
            ),
        ),
    ]
}
clocks = [s["clockDifferenceMs"] for s in samples if "clockDifferenceMs" in s]
result = {
    "coverage": {
        "hookEvents": len(rows),
        "sessions": len({r["session"] for r in rows}),
        "currentVisibleDom": sum(
            r.get("webviewToCurrentDomMs") is not None for r in rows
        ),
        "settledVisibleDom": sum(r["settledVisible"] for r in rows),
        "inputDelivery": sum(r["input-receivedMs"] is not None for r in rows),
    },
    "summary": summary,
    "stages": aggregate,
    "subsets": subset,
    "clockDifferenceMs": {
        "min": min(clocks, default=0),
        "max": max(clocks, default=0),
        "range": max(clocks, default=0) - min(clocks, default=0),
    },
    "slowestVisible": sorted(
        [r for r in rows if r.get("webviewToCurrentDomMs") is not None],
        key=lambda r: r["input-to-current-suggestions-domMs"],
        reverse=True,
    )[:8],
}
(out / "results.json").write_text(json.dumps(result, indent=2))
(out / "correlated-inputs.json").write_text(json.dumps(rows, indent=2))
print(
    json.dumps({"coverage": result["coverage"], "summary": result["summary"]}, indent=2)
)
