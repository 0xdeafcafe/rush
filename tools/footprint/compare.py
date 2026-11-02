#!/usr/bin/env python3
"""Compares a footprint run (measure.py's perf.json) with the last main
commit measured and with the baseline (DESIGN.md "Footprint watch"), and
writes what the workflow needs: the job summary, an issue's body, a
comment, and the next baseline.

    compare.py --new perf.json [--latest latest.json] [--baseline baseline.json]
               [--accept] --out DIR

DIR gets summary.md, issue.md, comment.md, closed.md, title.txt,
baseline.json and result.json ({"regressed": [...], "improved": [...]}).

The baseline is what a number is held to, so a slow creep is caught: a
number's baseline moves down when it gets better past its threshold, and
up only when the numbers are accepted (--accept: the issue was closed by
hand). Numbers new to the run join it as they are.
"""

import argparse
import json
import os

KIB = 1024
MIB = 1024 * 1024

# Per kind: (fraction, at least) a number must move by, both, to count;
# None is reported, never flagged.
THRESHOLDS = {
    "allocs": (0.05, 0),
    "bytes": (0.05, KIB),
    "heap": (0.05, 256 * KIB),
    "footprint": (0.10, MIB),
    "threads": (0.0, 0),
    "cpu": (0.50, 0.5),
    "wakeups": (0.50, 60),
    "binary": (0.05, 0),
    "info": None,
}

WHAT = {
    "allocs": "+5%",
    "bytes": "+5% and 1 KiB",
    "heap": "+5% and 256 KiB",
    "footprint": "+10% and 1 MiB",
    "threads": "any rise",
    "cpu": "+50% and 0.5 s/min",
    "wakeups": "+50% and 60/min",
    "binary": "+5%",
    "info": "not watched",
}


def load(path):
    if not path or not os.path.exists(path) or os.path.getsize(path) == 0:
        return None
    with open(path) as f:
        return json.load(f)


def fmt(kind, v):
    if v is None:
        return "–"
    if kind in ("heap", "bytes", "footprint", "binary"):
        a = abs(v)
        if a >= MIB:
            return f"{v / MIB:.2f} MiB"
        if a >= KIB:
            return f"{v / KIB:.1f} KiB"
        return f"{v:.0f} B"
    if kind == "cpu":
        return f"{v:.3f} s/min"
    if kind == "wakeups":
        return f"{v:.0f}/min"
    return f"{v:,.0f}"


def moved(kind, old, new):
    """'up' or 'down' when new is past the threshold from old, else ''."""
    t = THRESHOLDS.get(kind)
    if t is None or old is None or new is None:
        return ""
    frac, least = t
    d = new - old
    if d == 0:
        return ""
    big = abs(d) > least and (old == 0 or abs(d) / abs(old) > frac)
    if kind == "threads":
        big = True
    if not big:
        return ""
    return "up" if d > 0 else "down"


def change(old, new):
    if old is None or new is None:
        return "new" if old is None else "gone"
    d = new - old
    if old == 0:
        return "±0" if d == 0 else ("+" if d > 0 else "") + f"{d:g}"
    pct = 100.0 * d / old
    if d == 0:
        return "±0"
    return f"{'+' if d > 0 else ''}{pct:.1f}%"


def short(sha):
    return (sha or "")[:7]


def rows(old, new):
    """Each number's old value, new value and move."""
    om = (old or {}).get("metrics", {})
    nm = new.get("metrics", {})
    out = []
    for name in sorted(set(om) | set(nm)):
        o, n = om.get(name), nm.get(name)
        kind = (n or o)["kind"]
        ov = o["value"] if o else None
        nv = n["value"] if n else None
        out.append((name, kind, ov, nv, moved(kind, ov, nv)))
    return out


def table(rs, mark=True):
    lines = ["| what | before | after | change | |", "|---|---:|---:|---:|---|"]
    for name, kind, ov, nv, mv in rs:
        word = {"up": "**past threshold**", "down": "better"}.get(mv, "") if mark else ""
        if kind == "info":
            word = "not watched"
        lines.append(
            f"| {name} | {fmt(kind, ov)} | {fmt(kind, nv)} | {change(ov, nv)} | {word} |"
        )
    return "\n".join(lines)


def link(server, repo, a, b):
    if a == b or not a:
        return f"[`{short(b)}`]({server}/{repo}/commit/{b})" if repo else f"`{short(b)}`"
    if not repo:
        return f"`{short(a)}`..`{short(b)}`"
    if a == b:
        return f"[`{short(b)}`]({server}/{repo}/commit/{b})"
    return f"[`{short(a)}`..`{short(b)}`]({server}/{repo}/compare/{a}...{b})"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--new", required=True)
    ap.add_argument("--latest")
    ap.add_argument("--baseline")
    ap.add_argument("--accept", action="store_true")
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    server = os.environ.get("GITHUB_SERVER_URL", "https://github.com")
    repo = os.environ.get("GITHUB_REPOSITORY", "")
    run = os.environ.get("GITHUB_RUN_ID", "")
    run_link = f"{server}/{repo}/actions/runs/{run}" if repo and run else ""

    new = load(a.new)
    latest = load(a.latest)
    base = load(a.baseline) or latest
    sha = new.get("commit", "")

    # The new baseline, and what's past it.
    bm = {} if (a.accept or not base) else base.get("metrics", {})
    regressed, improved, next_base = [], [], {}
    base_sha = (base or {}).get("commit", "")
    for name, n in new["metrics"].items():
        b = bm.get(name)
        if b is not None and "commit" not in b:
            b = dict(b, commit=base_sha)
        mv = moved(n["kind"], b["value"] if b else None, n["value"])
        if mv == "up":
            regressed.append((name, n["kind"], b["value"], n["value"], mv, b.get("commit", "")))
            next_base[name] = b
        elif mv == "down":
            improved.append((name, n["kind"], b["value"], n["value"], mv, b.get("commit", "")))
            next_base[name] = dict(n, commit=sha)
        elif b is not None:
            next_base[name] = b
        else:
            next_base[name] = dict(n, commit=sha)
    baseline = {
        "commit": sha,
        "at": new.get("at", ""),
        "metrics": dict(sorted(next_base.items())),
        "flagged": [r[0] for r in regressed],
    }
    with open(os.path.join(a.out, "baseline.json"), "w") as f:
        json.dump(baseline, f, indent=1)
        f.write("\n")

    since = rows(latest, new)
    head = []
    if regressed:
        names = ", ".join(r[0] for r in regressed)
        its = "its threshold" if len(regressed) == 1 else "their thresholds"
        head.append(f"**{len(regressed)} past {its}:** {names}.")
    else:
        head.append("No number past its threshold.")
    if improved:
        head.append(f"Better past their thresholds: {', '.join(r[0] for r in improved)}.")
    if a.accept:
        head.append("The numbers were accepted: this run's are the new baseline.")
    errors = new.get("errors") or {}
    for proc, e in sorted(errors.items()):
        head.append(f"Not measured: **{proc}**: `{' '.join(str(e).split())[:300]}`")

    def flagged_table(rs):
        return table([(n, k, o, v, m) for n, k, o, v, m, _ in rs])

    old_sha = (latest or {}).get("commit", "")
    summary = ["## Footprint", "", "\n\n".join(head), ""]
    if regressed or improved:
        bs = sorted({r[5] for r in regressed + improved if r[5]})
        summary += [
            f"### Against the baseline ({', '.join(f'`{short(b)}`' for b in bs) or 'none'})",
            "",
            flagged_table(regressed + improved),
            "",
        ]
    summary += [
        f"### Since the last measured main commit ({link(server, repo, old_sha, sha) if latest else 'none yet'})",
        "",
        table(since),
        "",
        f"Idle numbers are over {new.get('window_s', 60)} s once settled; thresholds: "
        + "; ".join(f"{k} {v}" for k, v in WHAT.items() if k != "info")
        + ".",
        "",
    ]
    with open(os.path.join(a.out, "summary.md"), "w") as f:
        f.write("\n".join(summary))

    marker = "<!-- rush-footprint-watch -->"
    if regressed:
        first = regressed[0][5] or old_sha
        what = ", ".join(r[0] for r in regressed[:2])
        if len(regressed) > 2:
            what += f" and {len(regressed) - 2} more"
        title = f"Footprint went up: {what}"
        body = [
            marker,
            f"The footprint job measured numbers past their thresholds at {link(server, repo, sha, sha)}"
            + (f" ([run]({run_link}))" if run_link else "")
            + f", against the baseline; the change is in {link(server, repo, first, sha)}.",
            "",
            flagged_table(regressed),
            "",
            "Thresholds: " + "; ".join(f"{k} {v}" for k, v in WHAT.items() if k != "info") + ".",
            "",
            "Each run on main comments here while a number stays past its threshold, "
            "and closes this when they're all back under. Closing it by hand accepts "
            "the new numbers as the baseline. DESIGN.md \"Footprint watch\" says how "
            "each is measured.",
        ]
        comment = [
            f"Still past the thresholds at {link(server, repo, sha, sha)}"
            + (f" ([run]({run_link}))" if run_link else "")
            + ":",
            "",
            flagged_table(regressed),
        ]
    else:
        title, body, comment = "", [], []
    with open(os.path.join(a.out, "title.txt"), "w") as f:
        f.write(title)
    with open(os.path.join(a.out, "issue.md"), "w") as f:
        f.write("\n".join(body) + "\n")
    with open(os.path.join(a.out, "comment.md"), "w") as f:
        f.write("\n".join(comment) + "\n")
    was = (load(a.baseline) or {}).get("flagged", [])
    back = [r for r in rows(base, new) if r[0] in was]
    closed = [
        f"Back under the thresholds at {link(server, repo, sha, sha)}"
        + (f" ([run]({run_link}))" if run_link else "")
        + ":",
        "",
        table(back, mark=False) if back else "",
    ]
    with open(os.path.join(a.out, "closed.md"), "w") as f:
        f.write("\n".join(closed) + "\n")
    with open(os.path.join(a.out, "result.json"), "w") as f:
        json.dump(
            {
                "regressed": [r[0] for r in regressed],
                "improved": [r[0] for r in improved],
                "was_flagged": was,
            },
            f,
        )
    print("\n".join(head))


if __name__ == "__main__":
    main()
