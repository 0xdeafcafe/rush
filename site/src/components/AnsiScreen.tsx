// Static spans from a real tmux capture. No canvas, image, HTML injection or client parser.
import type { CSSProperties } from "react";
import { parseAnsi, cropRow, colours, type Cell } from "../lib/ansi";
export type Capture = { ansi: string; width: number; height: number };
export type Crop = { x: number; y: number; width: number; height: number };
export function rowsOf(capture: Capture, crop?: Crop) {
  const all = parseAnsi(capture.ansi);
  const area = crop ?? {
    x: 0,
    y: 0,
    width: capture.width,
    height: capture.height,
  };
  return Array.from({ length: area.height }, (_, y) =>
    cropRow(all[area.y + y] ?? [], area.x, area.width),
  );
}
const borders = /^[\u2500-\u259f]+$/u;
// Chrome is unselectable. Callers mark only content columns/rows as selectable.
export function AnsiLine({
  row,
  select = [],
}: {
  row: Cell[];
  select?: [number, number][];
}) {
  const runs: {
    text: string;
    width: number;
    style: CSSProperties;
    select: boolean;
    key: string;
  }[] = [];
  let column = 0;
  for (const cell of row) {
    const selectable =
      select.some(([start, end]) => column >= start && column < end) &&
      !borders.test(cell.text);
    const style: CSSProperties = {
      ...colours(cell.ink),
      fontWeight: cell.ink.bold ? 700 : 400,
      fontStyle: cell.ink.italic ? "italic" : undefined,
      textDecoration: cell.ink.underline ? "underline" : undefined,
    };
    const key = JSON.stringify([style, selectable]);
    const last = runs.at(-1);
    if (last?.key === key) {
      last.text += cell.text;
      last.width += cell.width;
    } else
      runs.push({
        text: cell.text,
        width: cell.width,
        style,
        select: selectable,
        key,
      });
    column += cell.width;
  }
  return (
    <>
      {runs.map((run, i) => (
        <span
          key={i}
          className={run.select ? "ansi-content" : undefined}
          style={{ ...run.style, width: `${run.width}ch` }}
        >
          {run.text}
        </span>
      ))}
    </>
  );
}
export default function AnsiScreen({
  capture,
  crop,
  label,
  contentRows,
  contentColumns,
  selection,
}: {
  capture: Capture;
  crop?: Crop;
  label: string;
  contentRows?: number[];
  contentColumns?: [number, number];
  selection?: Record<number, [number, number][]>;
}) {
  const rows = rowsOf(capture, crop),
    width = crop?.width ?? capture.width;
  return (
    <div className="ansi-container">
      <div
        className="ansi-scroll"
        tabIndex={0}
        role="region"
        aria-label={label}
      >
        <pre
          className="ansi-screen"
          style={{ "--cols": width } as CSSProperties}
        >
          {rows.map((row, i) => (
            <span className="ansi-row" key={i}>
              <AnsiLine
                row={row}
                select={
                  selection?.[i] ??
                  (contentRows?.includes(i)
                    ? [contentColumns ?? [0, width]]
                    : [])
                }
              />
              {"\n"}
            </span>
          ))}
        </pre>
      </div>
    </div>
  );
}
