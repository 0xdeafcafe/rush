// One label, rendered to HTML here and rasterised by bottle.ts for the mesh.
export default function Label() {
  return (
    <div className="label-landing">
      <div className="label-frame">
        <div data-label-face>
          <header>
            <div className="cap-band">
              <div className="band-run">
                <p>
                  GET THEM ALL IN.
                  <span className="band-tail" aria-hidden="true">
                    EVERY LAST AGENT.
                  </span>
                </p>
              </div>
              <span className="band-note">NOT FOR AGENT CONSUMPTION</span>
            </div>
            <div className="sheet">
              <nav className="nav" aria-label="Main navigation">
                <a href="#list">Read the label</a>
                <div className="nav-links">
                  <a href="#install">Install rush</a>
                  <a href="https://github.com/0xdeafcafe/rush">GitHub</a>
                </div>
              </nav>
            </div>
          </header>
          <div className="sheet">
            <section className="front" aria-label="rush, AI harness polisher">
              <div className="front-grid">
                <div className="identity">
                  <div className="batch">
                    <span>SOURCE BATCH</span>
                    <a
                      href="https://github.com/0xdeafcafe/rush/commit/98cec507f9d103d7f016b20bdc2cb95dfdb37153"
                      aria-label="Source commit 98cec507f9d103d7"
                    >
                      98CE-C507-F9D1-03D7
                    </a>
                  </div>
                  <h1 className="label-wordmark" aria-label="rush">
                    <svg viewBox="0 0 840 258" role="img" aria-label="RUSH">
                      <g transform="translate(53 18) skewX(-12)">
                        <path
                          fillRule="evenodd"
                          d="M0 0H135L170 34V100L137 132L180 224H113L78 144H58V224H0ZM58 49V96H105L116 85V60L105 49Z"
                        />
                        <path d="M190 0H250V163L263 176H294L307 163V0H367V180L323 224H234L190 180Z" />
                        <path d="M439 0H581V52H467L454 65V84L470 97H532L568 133V188L532 224H389V172H501L514 159V144H454L399 100V40Z" />
                        <path d="M598 0H656V81H712V0H770V224H712V137H656V224H598Z" />
                      </g>
                    </svg>
                  </h1>
                  <p className="product-type">AI HARNESS POLISHER</p>
                  <div className="front-copy">
                    <h2>Every coding agent you run. One terminal.</h2>
                    <p>
                      Claude Code, Codex, Copilot, Kimi and the rest, side by
                      side. Written in Rust.
                    </p>
                  </div>
                </div>
                <aside className="front-aside" aria-label="Read before use">
                  <div className="caution">
                    <div className="caution-inner">
                      <h2>
                        <svg
                          className="hazard"
                          viewBox="0 0 36 34"
                          aria-hidden="true"
                        >
                          <path
                            d="M18 3 34 31H2Z"
                            fill="none"
                            stroke="currentColor"
                            strokeWidth="3"
                          />
                          <path
                            d="M18 12V21M18 25V27"
                            stroke="currentColor"
                            strokeWidth="3"
                          />
                        </svg>
                        WARNING
                      </h2>
                      <p>
                        Apple silicon Macs only.
                        <br />
                        Install by the full name.
                      </p>
                      <p>
                        Plain “brew install rush” gets you GNU’s restricted
                        shell.
                      </p>
                      <p>
                        Costs are estimates.
                        <br />
                        Your bill may disagree.
                      </p>
                    </div>
                  </div>
                  <a className="aside-install" href="#install">
                    Install rush <span aria-hidden="true">↓</span>
                  </a>
                  <p className="aside-note fine">
                    Homebrew instructions below.
                  </p>
                </aside>
              </div>
              <div className="contents-line">
                <span>MULTI-HARNESS AGENT MIXTURE</span>
                <span>CONTENTS: ONE TERMINAL APP</span>
              </div>
            </section>
          </div>
        </div>
      </div>
    </div>
  );
}
