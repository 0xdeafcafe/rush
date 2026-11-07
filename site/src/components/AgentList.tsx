// rush's Agents screen as text, set from the capture docs/screenshots/agents.webp
// (rows, figures and colours as captured; the fleet concept's rebuild).
// Each line has two places: `was`, before the tax test asks its question,
// and `now`, the capture itself. The page renders `now`; scripts/motion.ts
// puts the list back to `was` until it's scrolled into view, then lets the
// question arrive and the row float up into Needs you.
import type { ReactNode } from "react";

type Metrics = [running: string, cpu: string, ram: string, tokens: string, cost: string, time: string];

type Line =
  | { kind: "group"; was: number; now: number; title: string; stats: ReactNode }
  | { kind: "project"; was: number; now: number; name: string; branch: string; worktree?: boolean }
  | { kind: "agent"; was: number; now: number; mark: ReactNode; name: ReactNode; latest: ReactNode; m: Metrics; hot?: number[]; tone?: "needs" | "done" }
  | { kind: "blank"; was: number; now: number };

// A span whose text changes when the question arrives.
const Swap = ({ was, now }: { was: ReactNode; now: ReactNode }) => (
  <>
    <span className="was">{was}</span>
    <span className="now">{now}</span>
  </>
);

const dot = <span className="t-gold">●</span>;
const star = <span className="t-orange">✳</span>;
const done = <span className="t-green">◆</span>;

const LINES: Line[] = [
  { kind: "group", was: 0, now: 0, title: "Needs you", stats: <Swap was="1 · $0.24" now="2 · $0.43" /> },
  { kind: "project", was: 1, now: 1, name: "checkout", branch: "main ±1 ↝2" },
  { kind: "agent", was: 2, now: 2, tone: "needs", mark: dot, name: "upgrade the storefront to React 19", latest: <span className="t-gold">usage limit · resets 20:00</span>, m: ["", "1%", "296M", "101k", "$0.24", "3h00m"] },
  { kind: "project", was: 9, now: 3, name: "tax-rounding", branch: "fix/tax-rounding", worktree: true },
  {
    kind: "agent",
    was: 10,
    now: 4,
    tone: "needs",
    mark: <Swap was={star} now={dot} />,
    name: "fix the flaky tax rounding test",
    latest: <Swap was="reading the rounding tests" now={<span className="t-gold">asks: Round half up, or half to even?</span>} />,
    m: ["", "1%", "332M", "105k", "$0.19", "35m"],
  },
  { kind: "blank", was: 3, now: 5 },
  { kind: "group", was: 4, now: 6, title: "Active", stats: <Swap was="8 · $6.08 · 344M idle ram" now="7 · $5.89 · 344M idle ram" /> },
  { kind: "project", was: 5, now: 7, name: "checkout", branch: "main ±1 ↝2" },
  { kind: "agent", was: 6, now: 8, tone: "done", mark: done, name: <>bump the checkout’s dependencies <span className="p-copilot">◈ copilot</span></>, latest: <><span className="t-green">just finished</span> · 18 packages updated, lockfile rewritten, tests pass</>, m: ["", "1%", "188M", "", "–", "48s"] },
  { kind: "project", was: 7, now: 9, name: "apple-pay", branch: "feat/apple-pay ±3", worktree: true },
  { kind: "agent", was: 8, now: 10, mark: star, name: "add Apple Pay to checkout", latest: "subagent: draft the rollout notes", m: ["↳1", "31%", "874M", "140k", "$2.62", "1h02m"] },
  { kind: "project", was: 11, now: 11, name: "design-system", branch: "main ±1" },
  { kind: "agent", was: 12, now: 12, mark: star, name: <>move the design tokens to CSS variables <span className="p-codex">◎ codex</span></>, latest: "running pnpm build:tokens", m: ["", "79%", "316M", "", "$3.10", "18m"], hot: [1] },
  { kind: "project", was: 13, now: 13, name: "field-notes", branch: "main" },
  { kind: "agent", was: 14, now: 14, tone: "done", mark: done, name: <>tidy the field notes README <span className="p-plain">▣ opencode</span></>, latest: "README reorganised under four headings", m: ["", "1%", "34M", "", "–", "48m"] },
  { kind: "project", was: 15, now: 15, name: "lumen-api", branch: "feat/rate-limits ±1" },
  { kind: "agent", was: 16, now: 16, tone: "done", mark: done, name: <>port the CSV importer to streams <span className="p-plain">◐ kimi</span></>, latest: "API error · Connection dropped", m: ["", "1%", "122M", "", "–", "9m"] },
  { kind: "agent", was: 17, now: 17, mark: star, name: "rate limit the public API", latest: "run the tests", m: ["", "248%", "662M", "107k", "$0.17", "26m"], hot: [1] },
  { kind: "project", was: 18, now: 18, name: "orbit-cli", branch: "main" },
  { kind: "agent", was: 19, now: 19, mark: star, name: <>document the orbit CLI’s flags <span className="p-gemini">✦ gemini</span></>, latest: "writing docs/flags.md", m: ["", "5%", "265M", "", "–", "11m"] },
];

const ROWS = 20;

function render(line: Line, i: number) {
  const style = { ["--was" as string]: line.was, ["--now" as string]: line.now };
  const moves = line.was !== line.now ? " moves" : "";
  switch (line.kind) {
    case "blank":
      return <li key={i} className="l" style={style} aria-hidden="true" />;
    case "group":
      return (
        <li key={i} className={"l l-group" + moves} style={style}>
          <b>▾ {line.title}</b> <span className="t-sub">{line.stats}</span>
          <i />
        </li>
      );
    case "project":
      return (
        <li key={i} className={"l l-project" + (line.worktree ? " l-worktree" : "") + moves} style={style}>
          {line.worktree && <span className="t-sub">⌥ </span>}
          <span className="t-blue">{line.name}</span> <span className="t-sub">{line.branch}</span>
          <i />
        </li>
      );
    case "agent":
      return (
        <li key={i} className={"l l-agent" + (line.tone ? ` l-${line.tone}` : "") + moves} style={style}>
          <span className="a-name">
            {line.mark} <b>{line.name}</b>
          </span>
          <span className="a-latest">{line.latest}</span>
          {line.m.map((v, j) => (
            <span key={j} className={"a-m" + (j === 0 ? " t-orange" : "") + (line.hot?.includes(j) ? " t-gold" : "")}>
              {v}
            </span>
          ))}
        </li>
      );
  }
}

export default function AgentList() {
  return (
    <div className="term" data-list>
      <p className="term-top">
        <b>rush</b>
        {"  "}
        <span className="t-gold">
          ● <Swap was="1" now="2" /> needs you
        </span>
        {"  "}
        <span className="t-orange">
          ✳ <Swap was="5" now="4" /> working
        </span>
        {"  "}
        <span className="t-sub">18 finished</span>
      </p>
      <div className="l-head" aria-hidden="true">
        <span>AGENTS▾</span>
        <span>LATEST</span>
        <span>RUNNING</span>
        <span>CPU</span>
        <span>RAM</span>
        <span>TOKENS</span>
        <span>COST</span>
        <span>TIME</span>
      </div>
      <ol className="lines" style={{ ["--rows" as string]: ROWS }}>
        {LINES.map(render)}
      </ol>
    </div>
  );
}
