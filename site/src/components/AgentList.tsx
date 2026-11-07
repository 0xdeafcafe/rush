// Both states are real installed-rush captures from tools/shots' fictional world.
// Matching rows retain their actual captured text and move between captured positions.
import type { CSSProperties } from "react";
import beforeANSI from "../screens/agents-before.ansi?raw";
import afterANSI from "../screens/agents.ansi?raw";
import beforeMeta from "../screens/agents-before.json";
import afterMeta from "../screens/agents.json";
import { rowsOf, AnsiLine } from "./AnsiScreen";
import { plain, type Cell } from "../lib/ansi";
const crop = { x: 0, y: 6, width: afterMeta.width, height: 24 };
const before = rowsOf({ ansi: beforeANSI, ...beforeMeta }, crop);
const after = rowsOf({ ansi: afterANSI, ...afterMeta }, crop);
function rowKey(row: Cell[], index: number, rows: Cell[][]): string {
  const text = plain(row);
  if (!text.trim())
    return (
      "space:" +
      (rows[index + 1] ? rowKey(rows[index + 1], index + 1, []) : "end")
    );
  const group = text.match(
    /(?:Needs you|Working|Idle|Just left|Today|Earlier)/,
  );
  if (group && /^\s*[┃▸]/.test(text)) return group[0];
  return text
    .replace(/^[\s┃▌│└├─·?✢✶◌]+/u, "")
    .split(/\s{2,}/)[0]
    .replace(/\s*✻.*$/, "")
    .replace(/^[│└─\s]+/, "");
}
const keys = before.map((row, i) => rowKey(row, i, before));
export default function AgentList() {
  return (
    <div className="ansi-container" data-list>
      <div
        className="ansi-scroll"
        tabIndex={0}
        role="region"
        aria-label="Captured rush Agents list: the tax rounding question moves into Needs you"
      >
        <pre
          className="ansi-screen ansi-moving"
          style={
            { "--cols": crop.width, "--rows": after.length } as CSSProperties
          }
        >
          {after.map((row, i) => {
            const previous = keys.indexOf(rowKey(row, i, after));
            const style = {
              "--was": previous < 0 ? i : previous,
              "--now": i,
              zIndex: plain(row).includes("tax rounding") ? 2 : undefined,
            } as CSSProperties;
            const content =
              i > 0 &&
              !/^[┃▸]?\s*(Needs you|Working|Idle|Just left|Today|Earlier)/.test(
                plain(row).trim(),
              );
            const select: [number, number] = content ? [6, 136] : [0, 0];
            return (
              <span
                key={i}
                className={"ansi-row" + (previous < 0 ? " ansi-new" : "")}
                style={style}
              >
                <span className="was">
                  {previous >= 0 && (
                    <AnsiLine row={before[previous]} select={[select]} />
                  )}
                </span>
                <span className="now">
                  <AnsiLine row={row} select={[select]} />
                </span>
                {"\n"}
              </span>
            );
          })}
        </pre>
      </div>
    </div>
  );
}
