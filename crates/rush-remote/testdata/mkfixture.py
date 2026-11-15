#!/usr/bin/env python3
"""Crafts a claude session with a long, varied transcript in a rush home.

Usage: mkfixture.py <rush_home> <acct_dir> <cwd>
Writes <rush_home>/sessions/rich1/{config,info}.json and a transcript of
~130 turns (text, shell and edit steps, todos, a subagent) under
<acct_dir>/projects/<slug-of-cwd>/<claude-session-id>.jsonl, as the claude
adapter finds one, plus that transcript's subagents/ beside it. Also turns
the feed on in <rush_home>/config.json.
"""
import json, os, sys

home, acct, cwd = sys.argv[1], sys.argv[2], sys.argv[3]
sid, csid = "rich1", "b19d3f2a-7777-4c4c-9a1a-000000000001"
d = os.path.join(home, "sessions", sid)
os.makedirs(d, exist_ok=True)

def slug(p):
    return "".join(c if c.isascii() and c.isalnum() else "-" for c in p)

proj = os.path.join(acct, "projects", slug(os.path.realpath(cwd)))
os.makedirs(proj, exist_ok=True)

def line(o):
    return json.dumps(o, separators=(",", ":"))

def user(text, t):
    return {"type": "user", "timestamp": t,
            "message": {"role": "user", "content": text}}

def assistant(parts, t, i, model="claude-sonnet-4-5", cost=0.0):
    return {"type": "assistant", "timestamp": t, "costUSD": cost,
            "message": {"id": f"m{i:04}", "role": "assistant", "model": model,
                        "content": parts}}

def text(s):
    return {"type": "text", "text": s}

def tool_use(id_, name, inp):
    return {"type": "tool_use", "id": id_, "name": name, "input": inp}

def tool_result(id_, out, err=False):
    return {"type": "tool_result", "tool_use_id": id_,
            "is_error": err, "content": out}

import time
t = time.time()
def ts():
    global t
    t += 7
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(t))

lines = []
for i in range(130):
    n = f"{i:03}"
    lines.append(line(user(f"question {n}: " + "explain ".join([f"point {k} of {n}" for k in range(6)]), ts())))
    if i % 7 == 3:
        lines.append(line(assistant([tool_use(f"call{n}", "Bash",
            {"command": f"echo step {n}\ncargo test -p rush-core", "description": f"test {n}"})], ts(), i)))
        lines.append(line(user([tool_result(f"call{n}", f"running 12 tests\ntest result: ok. 12 passed")], ts())))
    elif i % 7 == 5:
        lines.append(line(assistant([tool_use(f"call{n}", "Edit",
            {"file_path": f"src/file{n}.rs", "old_string": "fn old() {}", "new_string": "fn new() {\n    changed()\n}"})], ts(), i)))
        lines.append(line(user([tool_result(f"call{n}", f"the file `src/file{n}.rs` was changed")], ts())))
    elif i == 40:
        lines.append(line(assistant([tool_use(f"call{n}", "TodoWrite",
            {"todos": [{"content": f"first thing {n}", "status": "completed"},
                        {"content": "second thing", "status": "in_progress"},
                        {"content": "third thing", "status": "pending"}]})], ts(), i)))
        lines.append(line(user([tool_result(f"call{n}", "Todos have been modified successfully")], ts())))
    elif i == 80:
        lines.append(line(assistant([tool_use(f"call{n}", "Task",
            {"description": "find the paging bug", "subagent_type": "Explore",
             "prompt": "Search the repo for how pages are served and report the file paths."})], ts(), i)))
        lines.append(line(user([tool_result(f"call{n}", "Found: server.rs page_history, app.js page()")], ts())))
    elif i % 11 == 9:
        lines.append(line(assistant([tool_use(f"call{n}", "Bash",
            {"command": "exit 3"})], ts(), i)))
        lines.append(line(user([tool_result(f"call{n}", "the shell said: exit 3", err=True)], ts())))
    else:
        lines.append(line(assistant([text(f"answer {n}: " + " ".join(f"word{k}" for k in range(30)))], ts(), i)))

tr = os.path.join(proj, f"{csid}.jsonl")
with open(tr, "w") as f:
    f.write("\n".join(lines) + "\n")

# the subagent Claude Code would keep beside that transcript
subs = os.path.join(proj, csid, "subagents")
os.makedirs(subs, exist_ok=True)
with open(os.path.join(subs, "agent-abc.meta.json"), "w") as f:
    json.dump({"agentType": "Explore", "description": "find the paging bug",
               "model": "sonnet", "toolUseId": "call080", "spawnDepth": 1}, f)
with open(os.path.join(subs, "agent-abc.jsonl"), "w") as f:
    f.write(line({"type": "assistant", "timestamp": ts(),
                  "message": {"id": "sm1", "role": "assistant",
                              "content": [text("reading server.rs and app.js for the paging seam")]}}) + "\n")
    for k in range(40):
        f.write(line({"type": "assistant", "timestamp": ts(),
                      "message": {"id": f"sm{k+2:03}", "role": "assistant",
                                  "content": [text(f"subagent step {k}: still looking")]}}) + "\n")

with open(os.path.join(d, "config.json"), "w") as f:
    json.dump({"id": sid, "sessionId": csid, "resume": True, "cwd": cwd,
               "name": "paging probe",
               "account": {"name": "", "configDir": acct}}, f)

with open(os.path.join(d, "info.json"), "w") as f:
    json.dump({"id": sid, "kind": "claude", "sessionId": csid, "account": "",
               "cwd": cwd, "name": "paging probe", "hostPid": 0,
               "state": "stopped",
               "updatedAt": "2026-11-15T12:00:00Z"}, f)

# the feed on, as #feed on would have it
cfg = os.path.join(home, "config.json")
try:
    state = json.load(open(cfg))
except Exception:
    state = {}
state["twatter"] = True
with open(cfg, "w") as f:
    json.dump(state, f)

print(f"wrote {len(lines)} lines to {tr}")
print(f"session {sid} (claude {csid}), feed on")
