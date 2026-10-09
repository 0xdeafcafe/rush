# Review of the remote Rush plan

Review date: 2026-10-09. This is a detailed self-review of [the plan](remote-rush-plan.md), not an independent reviewer sign-off, code review of an implementation, penetration test, or deployment verification.

Repository baseline: `d45fca1ac995f880d699f78420443e8698c539f7`, including the existing dirty working tree observed during planning. The first complete plan draft had SHA-256 `5cb12d48358debe224429fe7818e607d13560663f2ab8ff464c045bca17a4be3`. Findings below were applied to the revised plan. Unrelated local work was not edited.

Final reviewed plan SHA-256: `761a9f40e09145d9fdd15e4689ed24154eac45bd2eee6323f36ef0693bd07b65`. Final checks confirmed all local document links resolve, balanced code fences, 10 unique phases, 20 unique implementation tasks, 40 unique acceptance scenarios, no unresolved placeholder markers, and no trailing whitespace in the two new documents. A final consistency pass also made the core remote-access switch close existing channels and made worker reloads respect the user's draft-persistence preference.

## Verdict

The architecture fits the requested outcome: existing session hosts keep execution local, a node makes that state controllable remotely, a hub aggregates machines, and native/web clients share one contract. Bundled, disabled-by-default Tailscale and Cloudflared plugins are explicit delivery requirements rather than hidden core dependencies.

The revised plan is sufficient to start the foundation phase once implementation is requested. It is **not** evidence of release readiness. Exact dependency/build costs, safe Serve lifecycle behavior, real mobile push, provider/platform support, and the user's DNS/certificate setup remain stated gates. These are not silently deferred until after public release.

The initial draft was weakest at crash boundaries and ownership: it named most risks but left some implementation-critical behavior implicit. The revised plan now specifies connection fencing, host journals, spawn locking/identity, storage failure handling, continuous attention observation, durable plugin disable intent, attachment confinement, and duplicate push behavior.

## Review method and coverage

1. Compared the plan against the user's original server/native-client and URL/web requests, then the added multi-machine/custom-domain/PWA and bundled opt-in plugin requirements.
2. Checked current process ownership, local protocol, replay, pending decisions, spawn identity, plugin validation/supervision, state storage, and UI coupling in the repository.
3. Compared the proposal with neighboring architecture documents: [multi-agent.md](multi-agent.md), [substrate.md](substrate.md), and [plugin architecture](../plugins/ARCHITECTURE.md). The new plan preserves neutral adapters and avoids making the inference proxy a dependency.
4. Walked normal, concurrent, disconnected, crashed, storage-failed, revoked, upgraded, and disabled states for each actor: host, node, hub, plugin, browser, native client, and phone push service.
5. Checked current official networking/browser documentation for Serve ownership, identity boundaries, DNS/certificate scope, named tunnel credentials, WebSocket reconnects, Access validation, and service-worker/push behavior. Sources are linked in the relevant plan sections.
6. Checked local Markdown links, acceptance/task identifiers, and document structure. No runtime feature tests are represented as completed.

## Requirement traceability

| User intent | Plan coverage | Evidence required before claiming it works |
| --- | --- | --- |
| Run Rush on a server and connect from local Rush | §§3–4, 8; P1–P4 | A02, A06, A08, A11–A12, A29 |
| Create/manage remote agents | §§4.1, 4.3–4.4, 8–9 | A03–A07, A15, A36 |
| Several laptops/servers in one target | §§3, 6.4, 7 | A03, A09–A10, A33, A35 |
| `rush.thuis.forbes.red` on a phone over Tailscale | §§6.2, 7, 9 | A17, A20, A22–A23 on actual deployment |
| Tailscale via a bundled opt-in plugin | §6.1–6.2; P5 | A01, A17–A18, A37 |
| Cloudflared via a bundled opt-in plugin | §6.1, 6.3–6.4; P8 | A01, A18–A21, A37–A38 |
| Slick, web-friendly versions of common Rush features | §9 feature matrix/journeys | P6 browser journeys; A04–A09, A16, A28, A32 |
| Notifications with app closed | §10; P7 | A22–A27, A35 on real devices |
| HTTPS and own hostname | §7 | Correct DNS resolution/certificate/authorization; no assumption that DNS supplies TLS |
| Keep it specialized and lightweight | §§3, 4, 9.4, 12–13 | Bounded state, selective streaming, embedded assets; measured performance rather than tmux claims |

## Findings and dispositions

### F01 — Stale node connections could remain command authorities

**Priority:** high. **Draft location:** §3.2. The draft named a connection generation but did not say how an old channel loses authority or how a cloned identity differs from a normal reconnect. A laptop changing networks can briefly have two live channels, making ordinary reconnection a concurrency problem.

**Revision:** hub-issued generation fencing, node instance lock, rejection of obsolete dispatch, explicit conflict for an independently running clone, and preservation of already accepted command outcomes. Added A33.

**Disposition:** addressed in plan; implementation verification remains P3/A33. No claim that fencing rolls back a previously executed action.

### F02 — Durable host deduplication had no concrete owner/store

**Priority:** high. **Draft location:** §§3.3, 4.3. The draft proposed a database per role and final-host deduplication but did not specify where hosts persist that information. Letting every detached host write the node's database would contradict the single-writer ownership and tie host availability to node storage.

**Evidence:** hosts are detached processes in [host/client.go](../internal/host/client.go); the node and hub do not exist today. Current [host/host.go](../internal/host/host.go) has no complete command acknowledgment/dedupe contract.

**Revision:** each host owns a bounded per-session command journal, surviving idle retirement; node/hub receipts mirror it. Added explicit expiry/epoch rejection, corruption handling, and A40. This still requires crash-consistent implementation and must not promise exactly-once provider effects.

**Disposition:** addressed in plan; P0/P2 must validate journal format/locking/compaction and fault boundaries.

### F03 — Spawn retries could overwrite identity or create another session

**Priority:** high. **Draft location:** §4.3. A reserved host ID alone did not establish a safe retry algorithm.

**Evidence:** `Config.fillIDs` assigns both IDs if `SessionID` is empty, and `Spawn` checks whether a host is currently alive before writing its configuration. A stopped session directory is not automatically safe to reuse. See [host/client.go](../internal/host/client.go).

**Revision:** reserve both IDs and command mapping under a creation lock, require matching creation ownership before reuse, preserve other stopped sessions on collision, and reconcile startup timeouts without allocating a replacement. Added A36.

**Disposition:** addressed in plan; P2 tests must include partial start and duplicate concurrent requests.

### F04 — Persistence failure could invalidate acknowledgment guarantees

**Priority:** high. **Draft location:** §4.3 and operation/rollback sections. The draft covered process crashes but not full disk, denied writes, corrupt journal, or migration failure. A durable-acceptance promise cannot survive ignoring those conditions.

**Revision:** reject before acceptance, mark post-dispatch uncertainty, stop unsafe retries, expose read-only/degraded service state, and preserve explicit local emergency controls independent of the hub. Added A34.

**Disposition:** addressed in plan; storage fault injection is a release gate, not an optional resilience improvement.

### F05 — Pending decision snapshots lacked retained source payloads

**Priority:** high. **Draft location:** §4.2. The desired authoritative snapshot existed on paper without explicitly accounting for the host's actual stored fields.

**Evidence:** [host/agent.go](../internal/host/agent.go) stores pending request markers and approval options; [host/host.go](../internal/host/host.go) has a bounded replay ring and a display `Needs` field. Those alone do not reconstruct every tool request or structured question after replay eviction. The current `answered` path also publishes resolution before the provider answer call finishes, already identified in the first draft.

**Revision:** retain complete live decision payloads outside the replay ring, snapshot them under the watermark, distinguish resolving/resolved/failed/unknown, and never resurrect an approval from an old transcript after incarnation loss.

**Disposition:** addressed in plan; A04–A05 and A08 must cover a pending request surviving ring eviction and an answer failing at the provider.

### F06 — Attention and notifications could depend on an open client

**Priority:** high. **Draft location:** §§4.2, 10. The draft required a headless hub but did not explicitly identify the continuous source of critical events while no client subscribes, or recovery when node→hub delivery is interrupted.

**Evidence:** current desktop notifications are triggered by `Model.notify` in [ui/model.go](../internal/ui/model.go). Session hosts produce events, but the proposed hub needs observation and catch-up ownership.

**Revision:** node watches active hosts independently of viewers; high-volume remote transcript streaming remains demand-driven. Added bounded critical-event journal, current-state reconciliation, gaps, and summarized catch-up notifications. Hub state transition/outbox insertion is transactional. Added A35.

**Disposition:** addressed in plan. Tests must prove a permanent watcher does not defeat idle host sleep and that reconnection does not push historical failures as new ones.

### F07 — Plugin disable could be undone by a crash during cleanup

**Priority:** high. **Draft location:** §6.1. A shutdown procedure without a persisted desired state can reconnect after a supervisor restart, reopening exposure the user just disabled.

**Evidence:** existing plugin supervision restarts active plugins with backoff in [plugind/runner.go](../internal/plugind/runner.go); optional on/off state exists in [plugin/bundled.go](../internal/plugin/bundled.go). Bundled process groups already exist in [plugin/sandbox.go](../internal/plugin/sandbox.go), so the plan should extend existing mechanisms, not claim there is no supervision.

**Revision:** persist disabled intent first, reconcile disabled resources only toward cleanup, version setup application, and distinguish transport ownership from the core remote-access kill switch. Added A37.

**Disposition:** addressed in plan. Exact graceful cleanup behavior on supported Tailscale versions remains a bounded probe; global reset is not an acceptable fallback.

### F08 — Public browser authentication did not define native-client behavior

**Priority:** medium/high. **Draft location:** §6.4. A terminal client with a Rush token can still be intercepted by a Cloudflare Access browser login. A WebSocket established before upstream token expiry can outlive that gate.

**Revision:** first-release native clients use the private origin; public-native access requires a separately tested supported Access flow or an explicit refusal. Node and browser ingress are separate, with no broad bypass of session APIs. Required Access token expiry bounds long-lived streams. Added A38 and documented revocation limits.

**Disposition:** addressed in plan. The transport-neutral core remains intact; this is a supported-deployment distinction, not an excuse to retry HTML as JSON indefinitely.

### F09 — Attachments were authorized but insufficiently bounded

**Priority:** high. **Draft location:** §§4.2, 5, 9.3. The draft promised image prompts but lacked staged-upload ownership, byte/pixel limits, cleanup, and a race-resistant filesystem boundary.

**Revision:** concrete starting limits, raster-only validated media, target-node staging, opaque scoped IDs, storage quotas/expiry, confined file opens, and explicit registration of created worktree roots. Added A39.

**Disposition:** addressed in plan. Implementation must test symlink replacement and compressed image expansion, not only reject `../` strings.

### F10 — Push language implied stronger delivery guarantees than available

**Priority:** medium. **Draft location:** §10/A22. A unique hub outbox key prevents some duplicate sends, but a push provider can accept a request whose response is lost. OS/browser behavior can also suppress or repeat delivery.

**Revision:** best-effort delivery, possible duplicates, bounded worker-side event-ID suppression, replacement tags, minimal metadata, and an acceptance scenario that does not demand exactly-once display. Explicitly preserved the distinction between push display off-tailnet and opening the private app.

**Disposition:** addressed in plan. Real-phone evidence remains mandatory, and server-side success is never displayed as proof the phone saw a notification.

### F11 — Web packaging and route fallback needed deploy-time behavior

**Priority:** medium. **Draft location:** §9.4. The first draft preserved `go install`, but did not explicitly prevent SPA fallback from turning API errors into HTML or handle old tabs requesting retired asset chunks.

**Revision:** route fallback limited to app navigation; API/auth responses stay structured; versioned old/new asset handling and deliberate refresh behavior. Embedded assets remain reproducible and ship without runtime Node.js.

**Disposition:** addressed in plan. A30 and A31 need clean-build and upgrade-browser checks, not just a successful frontend dev server.

### F12 — Early mobile spike could unnecessarily block all architecture work

**Priority:** medium. **Draft location:** P0 exit gate. Phone push feasibility was placed in P0 without accounting for unavailable devices or test HTTPS infrastructure.

**Revision:** use approved test infrastructure when available, but gate phone claims at P7 rather than blocking unrelated service contract work. Deployment questions remain explicit prerequisites for the affected phases.

**Disposition:** addressed in plan. No claim that missing phone access proves push will work.

## Existing design choices challenged and retained

**Why a hub rather than DNS load balancing?** The machines have different sessions. Round-robin DNS or several connectors to unrelated backends chooses a machine; it does not aggregate their state. Retain one hub, and defer HA/federation.

**Why not implement everything in the plugins?** Ordinary plugin ownership/network restrictions deliberately prevent broad session control. The core must provide the remote API and authorization; trusted connectivity code can use a bounded bundled role without widening third-party privileges.

**Why keep application authentication on Tailscale?** Tailnet access identifies a network peer, not necessarily the personal owner on every custom proxy path. Pairing offers a consistent baseline; correctly verified identity integration may reduce login friction later.

**Why not put all transcripts in the hub immediately?** That adds replication, retention, deletion, and privacy responsibilities before they are required. The retained design is explicit about unavailable offline history. This tradeoff must stay visible in UX; a later offline-history feature cannot be claimed as already provided.

**Why HTTP plus WebSocket?** It supplies a conventional command/resource interface and typed live events through both selected transport options. It avoids a custom tunnel protocol. Reliable replay and mutation outcomes are application responsibilities either way.

**Why SQLite plus host journals?** The hub needs atomic outbox/registry updates; detached execution owners need their own durable outcome boundary. This is more complexity than socket forwarding, but tied to explicit failure requirements. Keep journals minimal and benchmark dependencies; don't add an external database or full event-sourcing platform.

**Why phased terminal and web parity?** The TUI contains client-local operations that cannot safely be forwarded by changing a socket address. Explicit supported/unsupported operations prevent acting on the wrong filesystem while the common workflows are delivered.

## Remaining gates and limits

| Gate | Status | What closes it |
| --- | --- | --- |
| Store/WS build and binary cost | Unverified; P0 dependency gate | Clean builds, dependency/license review, measured footprint |
| Host outcome/snapshot protocol | Planned, not implemented | Crash/race/replay tests against actual detached hosts |
| Tailscale Serve ownership and cleanup | Unverified on installed versions | Real CLI tests alongside unrelated existing services |
| Cloudflare token/Access/certificate setup | Operator configuration unknown | Named tunnel, exact host certificate and unauthenticated/authenticated probes |
| User custom DNS/proxy | Unknown | Confirm authoritative DNS, routing, TLS termination and renewal |
| Mobile PWA/push | Real devices not tested | Installed app tests on promised phone/OS combinations |
| Provider/platform matrix | Not tested for this remote feature | Actual macOS/Linux managed sessions and capability exclusions |
| Performance budgets | Targets, not results | Controlled RTT/load/memory/reconnect measurements |
| Security boundaries | Design reviewed only | Implementation security review and adversarial integration tests before public exposure |

There are no known unresolved contradictions in the revised plan that prevent starting P0. The table above contains real gates: the plan must not be relabeled production-ready before they are closed. No implementation tests, benchmark, network change, tunnel activation, certificate issuance, or phone notification was performed during this planning task.
