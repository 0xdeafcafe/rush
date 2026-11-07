import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  parseAnsi,
  plain,
  colours,
  cropRow,
  DEFAULT_FG,
  DEFAULT_BG,
} from "./ansi.ts";
for (const [name, input, want] of [
  [
    "16 colours",
    "\x1b[31;44mX",
    { color: "#cd0000", backgroundColor: "#0000ee" },
  ],
  [
    "bright colours",
    "\x1b[93;104mX",
    { color: "#ffff00", backgroundColor: "#5c5cff" },
  ],
  [
    "256 cube",
    "\x1b[38;5;196;48;5;232mX",
    { color: "#ff0000", backgroundColor: "#080808" },
  ],
  [
    "truecolour",
    "\x1b[38;2;10;20;30;48;2;40;50;60mX",
    { color: "#0a141e", backgroundColor: "#28323c" },
  ],
  [
    "colon truecolour",
    "\x1b[38:2::10:20:30mX",
    { color: "#0a141e", backgroundColor: DEFAULT_BG },
  ],
  [
    "reverse",
    "\x1b[31;44;7mX",
    { color: "#0000ee", backgroundColor: "#cd0000" },
  ],
  [
    "reset",
    "\x1b[31;44;7m\x1b[0mX",
    { color: DEFAULT_FG, backgroundColor: DEFAULT_BG },
  ],
] as const)
  test(name, () => assert.deepEqual(colours(parseAnsi(input)[0][0].ink), want));
test("attributes reset independently, colours persist across rows", () => {
  const rows = parseAnsi("\x1b[1;2;3;4;7;31mA\n\x1b[22;23;24;27mB");
  assert.deepEqual(rows[0][0].ink, {
    fg: "#cd0000",
    bg: DEFAULT_BG,
    bold: true,
    dim: true,
    italic: true,
    underline: true,
    reverse: true,
  });
  assert.deepEqual(rows[1][0].ink, {
    fg: "#cd0000",
    bg: DEFAULT_BG,
    bold: false,
    dim: false,
    italic: false,
    underline: false,
    reverse: false,
  });
});
test("dim blends foreground, never fades background", () =>
  assert.deepEqual(colours(parseAnsi("\x1b[37;40;2mX")[0][0].ink), {
    color: "#7e7e7e",
    backgroundColor: "#000000",
  }));
test("OSC links remain inert text; HTML remains text", () =>
  assert.equal(
    plain(
      parseAnsi("\x1b]8;;javascript:alert(1)\x1b\\<script>\x1b]8;;\x1b\\")[0],
    ),
    "<script>",
  ));
test("wide and combining cells preserve terminal columns when cropped", () => {
  const row = parseAnsi("A界e\u0301🐓Z")[0];
  assert.deepEqual(
    row.map((c) => c.width),
    [1, 2, 1, 2, 1],
  );
  assert.equal(plain(cropRow(row, 2, 4)), " é🐓");
});
for (const name of [
  "agents",
  "agents-before",
  "subagents",
  "command-run",
  "feed",
])
  test(`${name} retains capture dimensions and strips controls`, () => {
    const data = readFileSync(
      new URL(`../screens/${name}.ansi`, import.meta.url),
      "utf8",
    );
    const meta = JSON.parse(
      readFileSync(new URL(`../screens/${name}.json`, import.meta.url), "utf8"),
    );
    const rows = parseAnsi(data);
    assert.equal(rows.length, meta.height);
    for (const row of rows) {
      assert.ok(row.reduce((n, c) => n + c.width, 0) <= meta.width);
      assert.ok(!plain(row).includes("\x1b"));
      assert.equal(
        cropRow(row, 0, meta.width).reduce((n, c) => n + c.width, 0),
        meta.width,
      );
    }
  });
