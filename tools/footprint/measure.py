#!/usr/bin/env python3
"""CI's footprint job (DESIGN.md "Footprint watch"): starts each long-lived
kind of rush process on the made-up home fixture.py wrote, lets it settle,
then measures it idle: physical footprint (/usr/bin/footprint), threads,
and CPU and wake-ups over a fixed window. Adds the counting allocator's
numbers the benches wrote (RUSH_BENCH_OUT) and the binary's size, and
writes them all as one perf.json.

    measure.py --rush BIN --root ROOT --bench-out FILE --out perf.json
               [--window 60] [--commit SHA]

macOS only (proc_pidinfo, proc_pid_rusage, footprint). Every process runs
in ROOT's home with nothing of the runner's: no RUSH_*, CLAUDE_* or token
variables, PATH the system's and the fake claude.
"""

import argparse
import ctypes
import ctypes.util
import json
import os
import re
import signal
import socket
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))

libc = ctypes.CDLL(ctypes.util.find_library("c"), use_errno=True)


class Timebase(ctypes.Structure):
    _fields_ = [("numer", ctypes.c_uint32), ("denom", ctypes.c_uint32)]


_tb = Timebase()
libc.mach_timebase_info(ctypes.byref(_tb))


def ticks_to_s(t):
    """rusage_info's times are mach ticks on Apple silicon."""
    return t * _tb.numer / _tb.denom / 1e9


def rusage(pid):
    """(cpu seconds, wake-ups, phys_footprint bytes) from
    proc_pid_rusage(RUSAGE_INFO_V4), or None."""
    buf = ctypes.create_string_buffer(512)
    if libc.proc_pid_rusage(pid, 4, buf) != 0:
        return None
    q = lambda off: int.from_bytes(buf.raw[off : off + 8], "little")
    # After the 16-byte uuid: user, system, pkg idle wake-ups, interrupt
    # wake-ups, pageins, wired, resident, phys_footprint.
    cpu = ticks_to_s(q(16) + q(24))
    wakes = q(32) + q(40)
    return cpu, wakes, q(72)


def threads(pid):
    """proc_pidinfo(PROC_PIDTASKINFO)'s pti_threadnum, or None."""
    buf = ctypes.create_string_buffer(96)
    if libc.proc_pidinfo(pid, 4, ctypes.c_uint64(0), buf, 96) != 96:
        return None
    return int.from_bytes(buf.raw[84:88], "little", signed=True)


def footprint(pid):
    """Physical footprint in bytes, as /usr/bin/footprint says."""
    try:
        out = subprocess.run(
            ["/usr/bin/footprint", "-p", str(pid), "-f", "bytes", "--noCategories"],
            capture_output=True,
            text=True,
            timeout=60,
        ).stdout
    except (OSError, subprocess.TimeoutExpired):
        return None
    m = re.search(r"Footprint:\s*(\d+)\s*B", out)
    return int(m.group(1)) if m else None


def ps_cpu(pid):
    """CPU time as ps says (10 ms steps): a check on rusage's units."""
    out = subprocess.run(
        ["ps", "-o", "time=", "-p", str(pid)], capture_output=True, text=True
    ).stdout.strip()
    m = re.match(r"(?:(\d+):)?(\d+):(\d+(?:\.\d+)?)", out)
    if not m:
        return None
    h, mi, s = m.groups()
    return int(h or 0) * 3600 + int(mi) * 60 + float(s)


def alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except OSError:
        return False


def wait_for(what, cond, timeout=90):
    """Polls cond until it holds: the event a process is ready on."""
    end = time.time() + timeout
    while time.time() < end:
        try:
            if cond():
                return True
        except OSError:
            pass
        time.sleep(0.2)
    print(f"  {what}: not ready after {timeout}s", file=sys.stderr)
    return False


def settle(pids, quiet=0.02, run=3, cap=60):
    """Waits until every process has used less than `quiet` CPU-seconds in
    each of `run` seconds running: done starting, whatever its startup
    does. At most cap seconds."""
    last = {p: (rusage(p) or (0, 0, 0))[0] for p in pids}
    calm = 0
    t0 = time.time()
    while calm < run and time.time() - t0 < cap:
        time.sleep(1)
        now = {p: (rusage(p) or (0, 0, 0))[0] for p in pids}
        busy = any(now[p] - last[p] >= quiet for p in pids)
        last = now
        calm = 0 if busy else calm + 1
    return time.time() - t0


def environment(root, home):
    env = {
        k: v
        for k, v in os.environ.items()
        if not re.match(r"(RUSH_|AGTOP_|CLAUDE|CODEX_|ANTHROPIC_|OPENAI_|GH_|GITHUB_TOKEN|TMUX)", k)
    }
    bin_ = os.path.join(root, "bin")
    tmp = os.path.join(root, "tmp")
    os.makedirs(bin_, exist_ok=True)
    os.makedirs(tmp, exist_ok=True)
    fake = os.path.join(bin_, "claude")
    if not os.path.exists(fake):
        os.symlink(os.path.join(HERE, "fakeclaude"), fake)
    config = os.path.join(home, ".config")
    env.update(
        HOME=home,
        RUSH_HOME=os.path.join(config, "rush"),
        RUSH_CACHE=os.path.join(home, "Library", "Caches", "rush"),
        XDG_CONFIG_HOME=config,
        XDG_CACHE_HOME=os.path.join(home, ".cache"),
        XDG_DATA_HOME=os.path.join(home, ".local", "share"),
        CODEX_HOME=os.path.join(home, ".codex"),
        TMPDIR=tmp + "/",
        PATH=bin_ + ":/usr/bin:/bin:/usr/sbin:/sbin",
    )
    return env


class Proc:
    def __init__(self, name, argv, env, cwd, logs, stdin=None):
        self.name = name
        self.out = os.path.join(logs, name.replace(" ", "-") + ".out")
        self.err = os.path.join(logs, name.replace(" ", "-") + ".err")
        self.p = subprocess.Popen(
            argv,
            env=env,
            cwd=cwd,
            stdin=stdin if stdin is not None else subprocess.DEVNULL,
            stdout=open(self.out, "w"),
            stderr=open(self.err, "w"),
            start_new_session=True,
        )
        self.pid = self.p.pid

    def text(self, which="out"):
        with open(self.out if which == "out" else self.err) as f:
            return f.read()


def start_all(rush, root, env, logs):
    """Starts each kind of process and waits for what says it's ready. The
    processes found, by name: their pids; and what went wrong."""
    home = env["HOME"]
    cwd = os.path.join(root, "work", "project-0")
    rush_home = env["RUSH_HOME"]
    pids, errors, keep = {}, {}, []

    # The plugin broker, with the bundled plugins: ready once its socket
    # answers.
    pd = Proc("plugind", [rush, "plugind"], env, cwd, logs)
    keep.append(pd)
    sock = os.path.join(rush_home, "plugins", "broker.sock")

    def broker_up():
        s = socket.socket(socket.AF_UNIX)
        try:
            s.connect(sock)
            return True
        finally:
            s.close()

    if wait_for("plugind", broker_up) and alive(pd.pid):
        pids["plugind"] = pd.pid
    else:
        errors["plugind"] = pd.text("err")[-400:] or "its socket never answered"

    # serve, on a port of its own: ready once it says where it listens.
    sv = Proc("serve", [rush, "serve", "--listen", "127.0.0.1:0"], env, cwd, logs)
    keep.append(sv)
    if wait_for("serve", lambda: "rush serve:" in sv.text("err")) and alive(sv.pid):
        pids["serve"] = sv.pid
    else:
        errors["serve"] = sv.text("err")[-400:] or "it never said it was listening"

    # A host, its agent the fake claude, in a git repo as most are: ready
    # once its first turn ends.
    git = ["git", "-c", "user.name=footprint", "-c", "user.email=footprint@rush.invalid"]
    if not os.path.isdir(os.path.join(cwd, ".git")):
        subprocess.run(git + ["init", "-q", "-b", "main"], cwd=cwd, env=env)
        subprocess.run(git + ["commit", "-q", "--allow-empty", "-m", "start"], cwd=cwd, env=env)
    prompt = os.path.join(root, "prompt.txt")
    with open(prompt, "w") as f:
        f.write("Say done.\n")
    st = subprocess.run(
        [
            rush,
            "session",
            "start",
            "--cwd",
            cwd,
            "--agent",
            "claude",
            "--binary",
            os.path.join(root, "bin", "claude"),
            "--name",
            "footprint",
            "--prompt-file",
            prompt,
            "--json",
        ],
        env=env,
        cwd=cwd,
        capture_output=True,
        text=True,
        timeout=120,
    )
    host_id = None
    try:
        info = json.loads(st.stdout)
        host_id = info.get("id")
    except ValueError:
        errors["host"] = (st.stderr or st.stdout)[-400:] or "session start said nothing"
    if host_id:
        try:
            subprocess.run(
                [rush, "session", "watch", host_id, "--until-idle"],
                env=env,
                cwd=cwd,
                capture_output=True,
                timeout=120,
            )
        except subprocess.TimeoutExpired:
            errors["host"] = "its first turn never ended"
        try:
            with open(os.path.join(rush_home, "sessions", host_id, "info.json")) as f:
                hp = int(json.load(f).get("hostPid") or 0)
        except (OSError, ValueError):
            hp = 0
        if hp and alive(hp):
            pids["host"] = hp
            errors.pop("host", None)
        else:
            errors.setdefault("host", "no host process")

    # The menu bar's feed: ready once it writes its first reading.
    feed = Proc("menubar feed", [rush, "menubar", "feed"], env, cwd, logs, stdin=subprocess.PIPE)
    keep.append(feed)
    if wait_for("menubar feed", lambda: feed.text().strip() != "") and alive(feed.pid):
        pids["menubar feed"] = feed.pid
    else:
        errors["menubar feed"] = feed.text("err")[-400:] or "it never wrote a reading"

    # The view, with no terminal (rush --soak), long enough to outlast the
    # window; ready when it has settled, below.
    ui = Proc("ui", [rush, "--soak", "600s", "200x50"], env, cwd, logs)
    keep.append(ui)
    time.sleep(1)
    if alive(ui.pid):
        pids["ui"] = ui.pid
    else:
        errors["ui"] = ui.text("err")[-400:] or "it ended"
    return pids, errors, keep, host_id


def stop_all(rush, env, keep, host_id, pids):
    if host_id:
        subprocess.run(
            [rush, "session", "stop", host_id], env=env, capture_output=True, timeout=60
        )
    for p in keep:
        if p.p.stdin:
            p.p.stdin.close()
        try:
            os.killpg(p.pid, signal.SIGTERM)
        except OSError:
            pass
    for pid in pids.values():
        try:
            os.kill(pid, signal.SIGTERM)
        except OSError:
            pass
    time.sleep(1)
    for p in keep:
        try:
            os.killpg(p.pid, signal.SIGKILL)
        except OSError:
            pass


def processes(rush, root, window, logs):
    home = os.path.join(root, "home")
    env = environment(root, home)
    metrics, notes = {}, {}
    pids, errors, keep, host_id = start_all(rush, root, env, logs)
    try:
        settled = settle(list(pids.values()))
        print(f"processes settled in {settled:.0f}s: {pids}", file=sys.stderr)
        t0 = time.time()
        first = {n: rusage(p) for n, p in pids.items()}
        ps0 = {n: ps_cpu(p) for n, p in pids.items()}
        counts = {n: [] for n in pids}
        while time.time() - t0 < window:
            time.sleep(5)
            for n, p in pids.items():
                c = threads(p)
                if c is not None:
                    counts[n].append(c)
        dt = time.time() - t0
        for n, p in pids.items():
            last = rusage(p)
            if not alive(p) or first[n] is None or last is None:
                errors[n] = "it ended during the window"
                continue
            per_min = 60.0 / dt
            cpu = (last[0] - first[n][0]) * per_min
            wakes = (last[1] - first[n][1]) * per_min
            fp = footprint(p) or last[2]
            metrics[f"{n}: footprint"] = {"kind": "footprint", "value": fp}
            if counts[n]:
                # The fewest seen: its long-lived threads, not a passing
                # worker's.
                metrics[f"{n}: threads"] = {"kind": "threads", "value": min(counts[n])}
            metrics[f"{n}: idle CPU"] = {"kind": "cpu", "value": round(cpu, 4)}
            metrics[f"{n}: idle wake-ups"] = {"kind": "wakeups", "value": round(wakes, 1)}
            metrics[f"{n}: CPU to start"] = {"kind": "info", "value": round(first[n][0], 3)}
            p1 = ps_cpu(p)
            notes[n] = {
                "threads seen": sorted(set(counts[n])),
                "ps cpu over the window": None if p1 is None or ps0[n] is None else round(p1 - ps0[n], 2),
            }
    finally:
        stop_all(rush, env, keep, host_id, pids)
    return metrics, errors, notes


def benches(path):
    """The counting allocator's numbers: name, kind and value a line."""
    out = {}
    if not path or not os.path.exists(path):
        return out
    with open(path) as f:
        for line in f:
            parts = line.rstrip("\n").split("\t")
            if len(parts) == 3:
                name, kind, v = parts
                out[name] = {"kind": kind, "value": float(v) if "." in v else int(v)}
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rush", required=True)
    ap.add_argument("--root", required=True)
    ap.add_argument("--bench-out")
    ap.add_argument("--out", required=True)
    ap.add_argument("--window", type=int, default=60)
    ap.add_argument("--commit", default=os.environ.get("GITHUB_SHA", ""))
    a = ap.parse_args()
    rush, root = os.path.abspath(a.rush), os.path.abspath(a.root)
    logs = os.path.join(root, "logs")
    os.makedirs(logs, exist_ok=True)
    metrics = benches(a.bench_out)
    metrics["binary: rush, release"] = {"kind": "binary", "value": os.path.getsize(rush)}
    procs, errors, notes = processes(rush, root, a.window, logs)
    metrics.update(procs)
    perf = {
        "commit": a.commit,
        "ref": os.environ.get("GITHUB_REF_NAME", ""),
        "run": os.environ.get("GITHUB_RUN_ID", ""),
        "at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "window_s": a.window,
        "metrics": dict(sorted(metrics.items())),
        "errors": errors,
        "notes": notes,
    }
    with open(a.out, "w") as f:
        json.dump(perf, f, indent=1)
        f.write("\n")
    print(json.dumps({"errors": errors, "notes": notes}, indent=1), file=sys.stderr)


if __name__ == "__main__":
    main()
