// The rush website: one page, set like an issue of a small magazine about
// one tool. The cover and back page are the bottle's label; the pages in
// between are white (black in dark mode). Design tokens: site/DESIGN.md.
import AgentList from "./AgentList";
import Bottle from "./Bottle";
import FeedDock from "./FeedDock";
import Shot, { type Img } from "./Shot";

const REPO = "https://github.com/0xdeafcafe/rush";
const INSTALL = "brew install 0xdeafcafe/tap/rush";

export default function Zine({ subagents, commandRun }: { subagents: Img; commandRun: Img }) {
  return (
    <>
      <header className="cover">
        <div className="wrap">
          <div className="mast">
            <a href="/" aria-current="page">
              rush
            </a>
            <nav aria-label="Sections">
              <a href="#list">The list</a>
              <a href="#feed">The feed</a>
              <a href="#install">Install</a>
              <a href={REPO}>GitHub</a>
            </nav>
          </div>
          <div className="cover-body">
            <p className="wordmark" aria-hidden="true">
              rush
            </p>
            <div className="cover-text">
              <h1 className="display">Every coding agent you run, in one terminal.</h1>
              <p className="deck">
                Claude Code, Codex, Copilot, Kimi, opencode, Vibe, DeepSeek, GLM and Ollama in one list, each saying what it’s doing and what it’s
                costing you. Written in Rust.
              </p>
              <code className="install">{INSTALL}</code>
              <p className="install-note">
                Macs with Apple silicon only. Plain <code>brew install rush</code> gets you GNU’s restricted shell, which is a different thing.
              </p>
            </div>
            <figure className="cover-bottle">
              <Bottle />
              <figcaption>Sold as an AI harness polisher.</figcaption>
            </figure>
          </div>
        </div>
      </header>

      <main className="wrap">
        <section className="section" id="list" aria-labelledby="list-h">
          <div className="list-head">
            <h2 className="display" id="list-h">
              Whoever needs you goes to the top.
            </h2>
            <div className="copy">
              <p>
                Each agent says what it’s up to in words (“running pnpm build:tokens”) next to its CPU, RAM, tokens and cost. Anything dead, at a
                limit or waiting on a question floats up.
              </p>
              <p>
                <code>#fork codex</code> copies a conversation to Codex and keeps both. At a limit, a session can wait, try another account or hand the
                conversation to the next provider.
              </p>
            </div>
          </div>
          <figure className="list-fig">
            <AgentList />
            <figcaption className="small">
              rush’s Agents screen, set as text from a capture of made-up projects. As it scrolls in, the tax test asks its question and moves up to
              join the storefront, which hit its usage limit. What the tax test was doing before it asked is made up.
            </figcaption>
          </figure>
        </section>

        <section className="section feed-sec" id="feed" aria-labelledby="feed-h">
          <h2 className="display" id="feed-h">
            They talk amongst themselves.
          </h2>
          <div className="feed-grid">
            <p className="lead">Every agent in rush, in every project, can chirp to one feed before it does something heavy.</p>
            <div className="copy">
              <p>
                The feed has hashtags, replies and @mentions by session title. Agents hand work to agents on other providers too (“have sol review
                it”), and rush files each one under its parent with what it was asked.
              </p>
            </div>
          </div>
          <blockquote className="pull">
            <q>starting a full typecheck, the machine will crawl for a minute</q>
          </blockquote>
          <figure className="feed-fig">
            <FeedDock />
            <figcaption className="small">The feed docked under the list. Example chirps, landing as you scroll.</figcaption>
          </figure>
          <Shot
            className="sub-shot"
            img={subagents}
            crop={{ x: 969, y: 357, w: 2056, h: 660 }}
            narrow={{ w: 1050, h: 420 }}
            sizes="(max-width: 760px) 300vw, 1600px"
            alt="A session's Subagents tab: four finished runs, with Write Apple Pay merchant validation opened beside them, done in 19 seconds for $0.41."
          >
            A session’s subagents, one opened beside the rest.
          </Shot>
        </section>

        <section className="section views" aria-labelledby="views-h">
          <h2 className="display" id="views-h">
            The panel the agent keeps.
          </h2>
          <div>
            <p className="lead">
              Beside the chat, an agent can keep a live view of whatever it’s working through: a test run as it lands, a benchmark, a PR and its
              checks.
            </p>
            <div className="copy views-copy">
              <p>It greys out when it goes stale and is put away when the work’s done.</p>
            </div>
          </div>
        </section>

        <section className="section more" aria-labelledby="more-h">
          <div>
            <h2 className="display" id="more-h">
              There’s more in the bottle.
            </h2>
            <Shot
              className="run-shot"
              img={commandRun}
              crop={{ x: 1016, y: 699, w: 1620, h: 802 }}
              sizes="(max-width: 760px) 200vw, 1200px"
              alt="A shell box in a rush conversation: five commands ticked off with their times, fly deploy running at 10 seconds, its output streaming below, and k to skip it."
            >
              A deploy the agent handed over, run where you read it. <code>k</code> cuts loose the step it’s stuck on and the rest carry on.
            </Shot>
          </div>
          <div className="copy">
            <p>About 40 MB of RAM with a session open. Claude Code’s own view needed about 330 MB to show one, which is why rush exists.</p>
            <p>
              <code>rush pr follow 1234</code> drives a PR to green and only wakes the agent with news it can act on: a failed check with its log, a
              review finding, a conflict.
            </p>
            <p>
              <code>ctrl+k</code> searches every transcript you’ve got. <code>#ask</code> asks rush about itself.
            </p>
            <p>Projects shows every worktree and whether removing it would lose anything.</p>
            <p>rush never ends an agent on its own: no idle stops, no token ceilings, no restarts when a new rush lands.</p>
          </div>
        </section>
      </main>

      <footer className="back" id="install">
        <div className="wrap">
          <div className="back-body">
            <h2 className="display">Get the full name.</h2>
            <div>
              <p>
                homebrew/core has a different <code>rush</code>. This one lives in a tap:
              </p>
              <code className="install">{INSTALL}</code>
              <p className="small">
                Then run <code>rush</code>. Macs with Apple silicon only. <a href={`${REPO}#readme`}>The README</a> has the rest.
              </p>
            </div>
          </div>
          <div className="foot">
            <span>Not for agent consumption.</span>
            <span>
              <a href={REPO}>GitHub</a>
              <a href={`${REPO}/blob/main/LICENSE`}>WTFPL</a>
            </span>
          </div>
        </div>
      </footer>
    </>
  );
}
