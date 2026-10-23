# The gate shares the machine like an OS, not a queue

Alex, 2026-10-12. Replaces the slot pool (Parallel, Stagger, Busy) in
`internal/gate`. Why: a first-come queue lets one long typecheck hold up
every agent, and a load-average wait at load 52 on 10 cores holds
everything forever. haven's own slot queue stacked on top of it, so a
job held haven's only slot while it waited in rush's.

## Rules

1. **CPU is never queued.** A gated run starts at once. The holder (the
   `rush gate run` / `rush gate hold` process that already stays alive
   beside the run) lowers the priority of the run's whole process tree as
   it ages: nice 0 at start, 5 after 30s of running, 10 after 2 min, 15
   after 10 min. It re-applies to new descendants every tick (setpriority
   per pid; children forked before a renice keep their old value). This is
   a multilevel feedback queue done with `renice`: short checks finish
   before anything gets pushed back; a long typecheck keeps running on
   spare CPU. Never `taskpolicy -b`: on Apple Silicon background QoS
   pins a process to the 2 efficiency cores.

2. **Memory is the only thing a run waits for, and waiting is fair by
   agent.** Each gated program has a memory estimate (defaults: tsc/tsgo
   4 GiB, vitest/jest 3 GiB, playwright 2 GiB, next/webpack/vite build
   3 GiB, go/cargo 2 GiB, lint tools 1 GiB; overridable in settings). A
   run starts when available memory (free + inactive + purgeable, from
   `vm_stat` / host_statistics64) minus a reserve (default 15% of RAM)
   covers its estimate, or when nothing else gated is running (one always
   may). Among waiters, the next to go is the one whose agent
   (`RUSH_SESSION`, else the parent agent pid) has the fewest gated runs
   right now; ties go to the oldest ticket. Each waiter works this out
   from the ticket files itself: still no daemon, still flock, still
   self-freeing when a holder dies. Tickets record the session id.

3. **A watchdog in the holder handles rogue runs**, every 2s:
   - wall-clock past its program's limit (default 30 min, settings
     override) → SIGTERM the tree, SIGKILL 10s later;
   - tree RSS above its program's cap (default 3× its estimate) → kill;
   - the agent that started it is gone (session pid dead, or the run
     re-parented to launchd with no live session) → kill;
   - memory pressure critical (`kern.memorystatus_vm_pressure_level` = 4)
     → the newest-started gated run on the machine SIGSTOPs its tree;
     it SIGCONTs when the level drops to normal (1). Only one run is
     paused per tick, newest first, so the oldest work finishes.
   Memory counts as critical too once `kern.memorystatus_level` (the
   share the kernel counts available) is at the floor (default 5%), and
   as a warning at twice that: on 2026-10-23 the machine hung and the
   kernel watchdog panicked with 14 MiB free while the pressure level
   still said normal, so nothing was paused.
   Every kill or pause tells the agent why (the existing inbox note the
   stuck-command watcher uses). A paused run is "waiting", never "hung".

4. **Gone:** Parallel, Stagger, Busy, the load-average wait, per-scope
   queues (memory is machine-wide, so one scope: the system). Settings
   that named them are read and ignored.

5. **`rush gate status`** shows each run: program, agent, age, nice,
   memory, and paused/waiting with the reason; and each waiter's place
   by the fair order.

6. **A guard stops what the gate never saw.** Not every heavy program
   comes through the gate: that night three `tsgo` language servers
   under the agents, none gated, held 16.7 GiB of 18. The gate plugin
   runs a guard (one per machine, by flock on `gate/guard`) that reads
   `kern.memorystatus_level` each second: one sysctl, no fork, so it
   still reads while the machine pages hard. At the floor it SIGTERMs
   the largest process under any agent (an agent is a process not
   rush's whose parent and grandparent are), gated or not, and SIGKILLs
   it 3s later. Never rush's own, never an agent's program itself,
   nothing under 256 MiB, and one at a time: the next only once the last
   is gone and a tick has passed. Its agent is told why. "Last resort"
   in the settings sets the floor, or turns the guard off.

## haven (langwatch repo)

haven is used by developers without rush, so it keeps its own check for
them. Under rush's gate (the session's NODE_OPTIONS carries rush's gate
preload, or `RUSH_GATE_HELD` is set) `haven machine run` and
`haven machine slot run` take no haven slot and run the command straight,
so rush's preload and shims are the one gate. haven's boot priority uses
`renice` instead of `taskpolicy -b`.
