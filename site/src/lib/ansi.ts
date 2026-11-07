// Build-time parser for tmux capture-pane -e. Never interprets links or HTML.
// A cell keeps its terminal width, including wide graphemes and combining marks.
export type Ink = {
  fg: string;
  bg: string;
  bold: boolean;
  dim: boolean;
  italic: boolean;
  underline: boolean;
  reverse: boolean;
};
export type Cell = { text: string; width: number; ink: Ink };
export type Frame = Cell[][];
export const DEFAULT_FG = "#f0ece6";
export const DEFAULT_BG = "#111111";
const fresh = (): Ink => ({
  fg: DEFAULT_FG,
  bg: DEFAULT_BG,
  bold: false,
  dim: false,
  italic: false,
  underline: false,
  reverse: false,
});
const ANSI16 = [
  "#000000",
  "#cd0000",
  "#00cd00",
  "#cdcd00",
  "#0000ee",
  "#cd00cd",
  "#00cdcd",
  "#e5e5e5",
  "#7f7f7f",
  "#ff0000",
  "#00ff00",
  "#ffff00",
  "#5c5cff",
  "#ff00ff",
  "#00ffff",
  "#ffffff",
];
const hex = (n: number) =>
  Math.max(0, Math.min(255, n)).toString(16).padStart(2, "0");
export function indexed(n: number): string {
  if (n < 16) return ANSI16[n] ?? DEFAULT_FG;
  if (n >= 232) return "#" + hex(8 + (Math.min(n, 255) - 232) * 10).repeat(3);
  const i = n - 16,
    level = [0, 95, 135, 175, 215, 255];
  return (
    "#" +
    [level[Math.floor(i / 36)], level[Math.floor(i / 6) % 6], level[i % 6]]
      .map(hex)
      .join("")
  );
}
function sgr(ink: Ink, parameters: string): Ink {
  const next = { ...ink };
  // Both semicolon SGR and ISO colon colours (38:2::r:g:b).
  const codes = parameters.split(";").flatMap((group) => {
    const parts = group.split(":").map((v) => Number(v || 0));
    if (
      (parts[0] === 38 || parts[0] === 48) &&
      parts[1] === 2 &&
      parts.length === 6
    )
      return [...parts.slice(0, 2), ...parts.slice(3)];
    if (parts[0] === 4 && parts.length > 1) return [parts[1] === 0 ? 24 : 4];
    return parts;
  });
  for (let i = 0; i < codes.length; i++) {
    const code = codes[i];
    if (code === 0) Object.assign(next, fresh());
    else if (code === 1) next.bold = true;
    else if (code === 2) next.dim = true;
    else if (code === 3) next.italic = true;
    else if (code === 4) next.underline = true;
    else if (code === 7) next.reverse = true;
    else if (code === 22) {
      next.bold = false;
      next.dim = false;
    } else if (code === 23) next.italic = false;
    else if (code === 24) next.underline = false;
    else if (code === 27) next.reverse = false;
    else if (code === 39) next.fg = DEFAULT_FG;
    else if (code === 49) next.bg = DEFAULT_BG;
    else if (code >= 30 && code <= 37) next.fg = indexed(code - 30);
    else if (code >= 40 && code <= 47) next.bg = indexed(code - 40);
    else if (code >= 90 && code <= 97) next.fg = indexed(code - 90 + 8);
    else if (code >= 100 && code <= 107) next.bg = indexed(code - 100 + 8);
    else if (code === 38 || code === 48) {
      const channel = code === 38 ? "fg" : "bg";
      const kind = codes[++i];
      if (kind === 5 && i + 1 < codes.length)
        next[channel] = indexed(codes[++i]);
      else if (kind === 2 && i + 3 < codes.length) {
        next[channel] =
          "#" +
          codes
            .slice(i + 1, i + 4)
            .map(hex)
            .join("");
        i += 3;
      }
    }
  }
  return next;
}
const graphemes = new Intl.Segmenter("en", { granularity: "grapheme" });
export function cellWidth(text: string): number {
  if (/^\p{Mark}+$/u.test(text)) return 0;
  const n = text.codePointAt(0) ?? 0;
  if (/\p{Emoji_Presentation}/u.test(text) || text.includes("\ufe0f")) return 2;
  return n >= 0x1100 &&
    (n <= 0x115f ||
      n === 0x2329 ||
      n === 0x232a ||
      (n >= 0x2e80 && n <= 0xa4cf && n !== 0x303f) ||
      (n >= 0xac00 && n <= 0xd7a3) ||
      (n >= 0xf900 && n <= 0xfaff) ||
      (n >= 0xfe10 && n <= 0xfe6f) ||
      (n >= 0xff00 && n <= 0xff60) ||
      (n >= 0xffe0 && n <= 0xffe6) ||
      n >= 0x20000)
    ? 2
    : 1;
}
export function parseAnsi(input: string): Frame {
  const rows: Frame = [[]];
  let ink = fresh(),
    column = 0;
  // OSC (including hyperlinks) is discarded, while its visible label remains text.
  const clean = input.replace(/\x1b\][\s\S]*?(?:\x07|\x1b\\)/g, "");
  const pieces = clean.split(/(\x1b\[[0-?]*[ -/]*[@-~]|\n|\r|\t)/);
  for (const part of pieces) {
    if (part.startsWith("\x1b[")) {
      if (part.endsWith("m")) ink = sgr(ink, part.slice(2, -1));
      continue;
    }
    if (part === "\n") {
      rows.push([]);
      column = 0;
      continue;
    }
    if (part === "\r") continue;
    const text =
      part === "\t"
        ? " ".repeat(8 - (column % 8))
        : part.replace(/[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]/g, "");
    for (const { segment } of graphemes.segment(text)) {
      const width = cellWidth(segment);
      rows.at(-1)!.push({ text: segment, width, ink });
      column += width;
    }
  }
  if (input.endsWith("\n")) rows.pop();
  return rows;
}
export function plain(row: Cell[]): string {
  return row.map((cell) => cell.text).join("");
}
export function cropRow(row: Cell[], start: number, width: number): Cell[] {
  const result: Cell[] = [];
  let x = 0,
    used = 0;
  for (const cell of row) {
    const end = x + cell.width;
    if (x >= start && end <= start + width) {
      result.push(cell);
      used += cell.width;
    } else if (end > start && x < start + width) {
      const count = Math.min(end, start + width) - Math.max(x, start);
      result.push({ text: " ".repeat(count), width: count, ink: cell.ink });
      used += count;
    }
    x = end;
  }
  if (used < width)
    result.push({
      text: " ".repeat(width - used),
      width: width - used,
      ink: fresh(),
    });
  return result;
}
export function colours(ink: Ink): { color: string; backgroundColor: string } {
  let fg = ink.reverse ? ink.bg : ink.fg;
  const bg = ink.reverse ? ink.fg : ink.bg;
  if (ink.dim)
    fg =
      "#" +
      [1, 3, 5]
        .map((i) =>
          hex(
            Math.round(
              parseInt(fg.slice(i, i + 2), 16) * 0.55 +
                parseInt(bg.slice(i, i + 2), 16) * 0.45,
            ),
          ),
        )
        .join("");
  return { color: fg, backgroundColor: bg };
}
