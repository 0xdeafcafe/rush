#!/usr/bin/env python3
"""Makes a home for CI's footprint job (DESIGN.md "Footprint watch"): Claude
Code transcripts under ~/.claude/projects and rush sessions under rush's
folder, the same every time (a fixed seed), so the numbers measured on it
move only when rush does.

    fixture.py ROOT

ROOT/home is the home (~/.claude, ~/.config/rush), ROOT/work the projects
the transcripts worked in, ROOT/big.jsonl names the largest transcript (the
conversation the open bench reads). Only the files' times follow the clock:
the list shows what changed lately.
"""

import json
import os
import random
import sys
import time

PROJECTS = 12
# Transcripts, and turns in each.
SMALL = 240
SMALL_TURNS = 12
# The long conversation the open bench reads, and its subagent runs.
BIG_TURNS = 1500
SUBAGENTS = 6
SUBAGENT_TURNS = 60
# rush's own sessions, stopped (as rush-remote's tests/listing.rs).
RUSH_SESSIONS = 400

WORDS = (
    "the host module keeps a replay of every event so a view that attaches "
    "late sees the whole turn and the list reads info json for each session "
    "upgrade migration deprecation service entitlement usage plugin broker "
    "menu bar feed serve remote session worktree branch commit test build "
    "allocation heap footprint thread wake idle render frame conversation"
).split()

MODELS = ("claude-opus-5-5", "claude-sonnet-4-6", "claude-haiku-4-5-20251001")
TOOLS = ("Bash", "Read", "Edit", "Grep")


def words(r, n):
    return " ".join(r.choice(WORDS) for _ in range(n))


def uuid(r):
    h = "%032x" % r.getrandbits(128)
    return f"{h[:8]}-{h[8:12]}-4{h[13:16]}-8{h[17:20]}-{h[20:]}"


def stamp(t):
    return time.strftime("%Y-%m-%dT%H:%M:%S", time.gmtime(t)) + ".000Z"


def project_dir_name(cwd):
    # Claude Code's: every character not a letter or digit becomes '-'.
    return "".join(c if c.isalnum() else "-" for c in cwd)


def transcript(r, path, cwd, sid, turns, start, sidechain=False, agent=""):
    """Writes a conversation of turns: a prompt, a tool call and its result,
    then the answer."""
    parent = None
    t = start
    model = r.choice(MODELS)
    with open(path, "w") as f:

        def line(kind, message, **extra):
            nonlocal parent, t
            u = uuid(r)
            d = {
                "parentUuid": parent,
                "isSidechain": sidechain,
                "userType": "external",
                "cwd": cwd,
                "sessionId": sid,
                "version": "2.1.306",
                "gitBranch": "main",
                "type": kind,
                "uuid": u,
                "timestamp": stamp(t),
                "message": message,
            }
            if agent:
                d["agentId"] = agent
            d.update(extra)
            f.write(json.dumps(d, separators=(",", ":")) + "\n")
            parent = u
            t += r.randint(2, 40)

        for i in range(turns):
            line("user", {"role": "user", "content": words(r, r.randint(8, 60))})
            tool = r.choice(TOOLS)
            tid = "toolu_%024x" % r.getrandbits(96)
            usage = {
                "input_tokens": r.randint(3, 40),
                "output_tokens": r.randint(20, 900),
                "cache_read_input_tokens": r.randint(1000, 90000),
                "cache_creation_input_tokens": r.randint(0, 9000),
            }
            mid = "msg_%024x" % r.getrandbits(96)
            line(
                "assistant",
                {
                    "id": mid,
                    "type": "message",
                    "role": "assistant",
                    "model": model,
                    "content": [
                        {"type": "text", "text": words(r, r.randint(10, 80))},
                        {
                            "type": "tool_use",
                            "id": tid,
                            "name": tool,
                            "input": {
                                "command": "cargo test -p rush-host " + words(r, 3),
                                "description": words(r, 5),
                            }
                            if tool == "Bash"
                            else {"file_path": f"{cwd}/src/{r.choice(WORDS)}.rs"},
                        },
                    ],
                    "stop_reason": "tool_use",
                    "stop_sequence": None,
                    "usage": usage,
                },
                requestId="req_%024x" % r.getrandbits(96),
            )
            out = "\n".join(words(r, 12) for _ in range(r.randint(1, 40)))
            line(
                "user",
                {
                    "role": "user",
                    "content": [
                        {
                            "tool_use_id": tid,
                            "type": "tool_result",
                            "content": out,
                            "is_error": False,
                        }
                    ],
                },
                toolUseResult={"stdout": out, "stderr": "", "interrupted": False},
            )
            line(
                "assistant",
                {
                    "id": "msg_%024x" % r.getrandbits(96),
                    "type": "message",
                    "role": "assistant",
                    "model": model,
                    "content": [{"type": "text", "text": words(r, r.randint(20, 200))}],
                    "stop_reason": "end_turn",
                    "stop_sequence": None,
                    "usage": usage,
                },
                requestId="req_%024x" % r.getrandbits(96),
            )
    return t


def main():
    if len(sys.argv) != 2:
        sys.exit("usage: fixture.py ROOT")
    root = os.path.abspath(sys.argv[1])
    home = os.path.join(root, "home")
    r = random.Random(20261102)
    now = time.time()
    projects = os.path.join(home, ".claude", "projects")
    cwds = []
    for p in range(PROJECTS):
        cwd = os.path.join(root, "work", f"project-{p}")
        os.makedirs(cwd, exist_ok=True)
        os.makedirs(os.path.join(projects, project_dir_name(cwd)), exist_ok=True)
        cwds.append(cwd)
    for i in range(SMALL):
        cwd = cwds[i % PROJECTS]
        sid = uuid(r)
        path = os.path.join(projects, project_dir_name(cwd), sid + ".jsonl")
        # Spread over the last ten days, the newest last.
        start = now - (SMALL - i) * 3600
        transcript(r, path, cwd, sid, SMALL_TURNS, start)
        os.utime(path, (start + 600, start + 600))
    cwd = cwds[0]
    sid = uuid(r)
    big = os.path.join(projects, project_dir_name(cwd), sid + ".jsonl")
    transcript(r, big, cwd, sid, BIG_TURNS, now - 86400)
    subs = os.path.join(big[: -len(".jsonl")], "subagents")
    os.makedirs(subs, exist_ok=True)
    for j in range(SUBAGENTS):
        agent = "%016x" % r.getrandbits(64)
        transcript(
            r,
            os.path.join(subs, f"agent-{agent}.jsonl"),
            cwd,
            sid,
            SUBAGENT_TURNS,
            now - 80000,
            sidechain=True,
            agent=agent,
        )
    os.utime(big, (now - 3600, now - 3600))
    with open(os.path.join(root, "big.jsonl"), "w") as f:
        f.write(big + "\n")
    sessions = os.path.join(home, ".config", "rush", "sessions")
    for i in range(RUSH_SESSIONS):
        id_ = "%08x" % (0xF0000000 + i)
        d = os.path.join(sessions, id_)
        os.makedirs(d, exist_ok=True)
        info = {
            "id": id_,
            "sessionId": uuid(r),
            "account": "default",
            "cwd": cwds[i % PROJECTS],
            "name": f"fix the flaky upgrade test in the host module {i}",
            "hostPid": 0,
            "state": "stopped",
            "model": r.choice(MODELS),
        }
        with open(os.path.join(d, "info.json"), "w") as f:
            json.dump(info, f)
    print(f"fixture: {SMALL + 1} transcripts, {RUSH_SESSIONS} rush sessions in {home}")


if __name__ == "__main__":
    main()
