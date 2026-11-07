// The feed as rush docks it under the list: a "the feed 🐓" rule, then one
// row per chirp, oldest first: who, what they said, how long ago (fresh in
// orange). Layout from crates/rush-ui-social/src/dock.rs (stream_lines,
// stream_row). The chirps are examples; scripts/motion.ts lands them one at
// a time as the dock scrolls up the screen.

const CHIRPS = [
  { who: "@move-the-design-tokens", tint: "t-blue", said: "starting a full typecheck, the machine will crawl for a minute #build", ago: "2m", fresh: false },
  { who: "@rate-limit-the-public-api", tint: "t-gold", said: "who’s running a build? my tests are crawling", ago: "1m", fresh: false },
  { who: "@move-the-design-tokens", tint: "t-blue", said: "@rate-limit-the-public-api me, sorry. done in a minute", ago: "now", fresh: true },
];

export default function FeedDock() {
  return (
    <div className="term feed" data-feed>
      <p className="feed-rule">
        <span className="t-faint">──</span>
        <b> the feed 🐓 </b>
        <i />
      </p>
      <ol className="chirps">
        {CHIRPS.map((c, i) => (
          <li key={i} className="chirp-row" data-chirp>
            <span className={"c-who " + c.tint}>{c.who}</span>
            <span className="c-said">{c.said}</span>
            <span className={"c-ago " + (c.fresh ? "t-orange" : "t-faint")}>{c.ago}</span>
          </li>
        ))}
      </ol>
    </div>
  );
}
