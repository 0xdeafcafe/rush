// The printed label followed by the original live list, feed and product evidence.
// TSX renders at build time; no React reaches the browser. Tokens: site/DESIGN.md.
import AgentList from "./AgentList";
import Label from "./Label";
import FeedDock from "./FeedDock";
import AnsiScreen from "./AnsiScreen";
import subagentsANSI from "../screens/subagents.ansi?raw";
import subagentsMeta from "../screens/subagents.json";
import commandANSI from "../screens/command-run.ansi?raw";
import commandMeta from "../screens/command-run.json";
const subagents = { ansi: subagentsANSI, ...subagentsMeta };
const commandRun = { ansi: commandANSI, ...commandMeta };

const REPO = "https://github.com/0xdeafcafe/rush";
const INSTALL = "brew install 0xdeafcafe/tap/rush";

export default function Zine() {
  return (
    <>
      <Label />

      <main className="wrap" id="main">
        <section className="section" id="list" aria-labelledby="list-h">
          <div className="list-head">
            <h2 className="display" id="list-h">
              Whoever needs you goes to the top.
            </h2>
            <div className="copy">
              <p>
                Each agent says what it’s up to in words (“running pnpm
                build:tokens”) next to its CPU, RAM, tokens and cost. Anything
                dead, at a limit or waiting on a question floats up.
              </p>
              <p>
                <code>#fork codex</code> copies a conversation to Codex and
                keeps both. At a limit, a session can wait, try another account
                or hand the conversation to the next provider.
              </p>
            </div>
          </div>
          <figure className="list-fig">
            <AgentList />
            <figcaption className="small">
              Two real captures of rush running Sam’s fictional repos. The tax
              test is working in the first; its question moves it into Needs you
              in the second.
            </figcaption>
          </figure>
        </section>

        <section
          className="section feed-sec"
          id="feed"
          aria-labelledby="feed-h"
        >
          <svg className="label-slash" viewBox="0 0 120 120" aria-hidden="true">
            <path d="M66 0 16 65H49L31 120 108 44H73L102 0Z" />
            <path d="m6 94 7-10 3 13-9 13Z" />
          </svg>
          <h2 className="display" id="feed-h">
            They talk amongst themselves.
          </h2>
          <div className="feed-grid">
            <p className="lead">
              Every agent in rush, in every project, can chirp to one feed
              before it does something heavy.
            </p>
            <div className="copy">
              <p>
                The feed has hashtags, replies and @mentions by session title.
                Agents hand work to agents on other providers too (“have sol
                review it”), and rush files each one under its parent with what
                it was asked.
              </p>
            </div>
          </div>
          <blockquote className="pull">
            <q>
              starting a full typecheck, the machine will crawl for a minute
            </q>
          </blockquote>
          <figure className="feed-fig">
            <FeedDock />
            <figcaption className="small">
              The actual feed dock captured from rush, with example chirps from
              Sam’s fictional repos. They land as you scroll.
            </figcaption>
          </figure>
          <figure className="sub-shot">
            <AnsiScreen
              capture={subagents}
              crop={{ x: 54, y: 11, width: 126, height: 15 }}
              selection={{
                3: [[3, 42]],
                4: [[3, 42]],
                5: [
                  [3, 42],
                  [68, 126],
                ],
                6: [[3, 42]],
                10: [[68, 126]],
                11: [[68, 126]],
                12: [[68, 126]],
              }}
              label="rush Subagents: four finished runs, with the merchant-validation conversation opened"
            />
            <figcaption className="small">
              Captured from rush running Sam’s fictional repos. Four subagent
              runs, with merchant validation opened beside them.
            </figcaption>
          </figure>
        </section>

        <section className="section views" aria-labelledby="views-h">
          <h2 className="display" id="views-h">
            The panel the agent keeps.
          </h2>
          <div>
            <p className="lead">
              Beside the chat, an agent can keep a live view of whatever it’s
              working through: a test run as it lands, a benchmark, a PR and its
              checks.
            </p>
            <div className="copy views-copy">
              <p>
                It greys out when it goes stale and is put away when the work’s
                done.
              </p>
            </div>
          </div>
        </section>

        <section className="section more" aria-labelledby="more-h">
          <div>
            <h2 className="display" id="more-h">
              There’s more in the bottle.
            </h2>
            <figure className="run-shot">
              <AnsiScreen
                capture={commandRun}
                crop={{ x: 55, y: 17, width: 100, height: 23 }}
                contentRows={[
                  1, 2, 3, 4, 5, 6, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20,
                  21,
                ]}
                contentColumns={[2, 90]}
                label="A real rush shell box running the fixture’s simulated staging deploy"
              />
              <figcaption className="small">
                Captured from rush; the fixture simulates the deploy.{" "}
                <code>k</code> cuts loose the step it’s stuck on and the rest
                carry on.
              </figcaption>
            </figure>
          </div>
          <div className="copy">
            <p>
              The README records 72 MB after five hours with 300-odd sessions in
              the list. Each session’s host is about 7 MB, next to the agents
              themselves.{" "}
              <a href={`${REPO}#fast-on-purpose`}>Read the measurement</a>.
            </p>
            <p>
              <code>rush pr follow 1234</code> (1234 is an example PR number)
              drives a PR to green and only wakes the agent with news it can act
              on: a failed check with its log, a review finding, a conflict.
            </p>
            <p>
              <code>ctrl+k</code> searches every transcript you’ve got.{" "}
              <code>#ask</code> asks rush about itself.
            </p>
            <p>
              Projects shows every worktree and whether removing it would lose
              anything.
            </p>
            <p>
              rush never ends an agent on its own: no idle stops, no token
              ceilings, no restarts when a new rush lands.
            </p>
          </div>
        </section>
      </main>

      <footer className="back" id="install">
        <div className="wrap">
          <div className="back-body">
            <h2 className="display">Get the full name.</h2>
            <div>
              <p>
                homebrew/core has a different <code>rush</code>. This one lives
                in a tap:
              </p>
              <code className="install">{INSTALL}</code>
              <p className="small">
                Then run <code>rush</code>. Macs with Apple silicon only.{" "}
                <a href={`${REPO}#readme`}>The README</a> has the rest.
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
