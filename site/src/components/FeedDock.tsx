// The actual feed dock, captured by tmux from tools/shots' fictional board.
import type { CSSProperties } from "react";
import ansi from "../screens/feed.ansi?raw";
import meta from "../screens/feed.json";
import { parseAnsi, plain, cropRow } from "../lib/ansi";
import { AnsiLine } from "./AnsiScreen";
const all = parseAnsi(ansi);
const start = all.findIndex((row) => plain(row).includes("the feed"));
if (start < 0) throw new Error("The feed capture has no feed dock");
const rows = all
  .slice(start, start + 4)
  .map((row) => cropRow(row, 0, meta.width));
export default function FeedDock() {
  return (
    <div className="ansi-container" data-feed>
      <div
        className="ansi-scroll"
        tabIndex={0}
        role="region"
        aria-label="Captured feed with three example chirps"
      >
        <pre
          className="ansi-screen"
          style={{ "--cols": meta.width } as CSSProperties}
        >
          {rows.map((row, i) => (
            <span
              key={i}
              className={"ansi-row" + (i ? " chirp-row" : "")}
              data-chirp={i ? true : undefined}
            >
              <AnsiLine row={row} select={i ? [[1, meta.width - 5]] : []} />
              {"\n"}
            </span>
          ))}
        </pre>
      </div>
    </div>
  );
}
