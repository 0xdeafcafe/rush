// Run against `site/dist` served over HTTP. Browser downloads and evidence stay
// under TMPDIR; see MOBILE-REVIEW.md for the exact commands and device limits.
import assert from "node:assert/strict";
import { mkdir, writeFile, readFile } from "node:fs/promises";
import { join } from "node:path";
import { chromium, webkit, devices } from "playwright";
import { PNG } from "pngjs";

assert(process.env.TMPDIR, "Set TMPDIR for browser evidence");
const output = join(process.env.TMPDIR, "rush-mobile-check");
await mkdir(output, { recursive: true });
const base = process.env.BOTTLE_URL ?? "http://127.0.0.1:8768";
// Playwright 1.55.1 has iPhone 15 but no Pixel 8 registry entry. Explicit Pixel 8
// screen/DPR/touch settings, with the registry's Android Chrome UA as the base.
const pixel8 = {
  ...devices["Pixel 7"],
  userAgent: devices["Pixel 7"].userAgent.replace("Pixel 7", "Pixel 8"),
  viewport: { width: 412, height: 839 },
  screen: { width: 412, height: 915 },
  deviceScaleFactor: 2.625,
};
const mobile = [
  ["iphone15", webkit, devices["iPhone 15"]],
  ["pixel8", chromium, pixel8],
];
const desktop = [
  [1440, 900],
  [1920, 1080],
  [2560, 1440],
  [1576, 2446],
  [390, 844],
].map(([width, height]) => [
  `${width}x${height}`,
  chromium,
  { viewport: { width, height } },
]);
const results = [];
const save = () =>
  writeFile(join(output, "results.json"), JSON.stringify(results, null, 2));
const settle = (page) =>
  page.evaluate(
    () =>
      new Promise((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(resolve)),
      ),
  );
const state = (page) =>
  page.locator("#bottle-intro").evaluate((el) => ({ ...el.dataset }));
async function ready(page) {
  await page.goto(base);
  await page.waitForFunction(
    () => {
      const data = document.querySelector("#bottle-intro").dataset;
      return data.mode === "webgl" || data.fallback;
    },
    {},
    { timeout: 45000 },
  );
}
async function scroll(page, progress) {
  await page.evaluate((p) => {
    const travel =
      document.querySelector("#label-page").getBoundingClientRect().top +
      scrollY;
    scrollTo(0, p === 1 ? Math.ceil(travel) : travel * p);
  }, progress);
  await settle(page);
}
async function difference(before, after) {
  const a = PNG.sync.read(await readFile(before)),
    b = PNG.sync.read(await readFile(after));
  assert.equal(a.width, b.width);
  assert.equal(a.height, b.height);
  let changed = 0,
    total = 0,
    max = 0;
  for (let i = 0; i < a.data.length; i += 4) {
    let any = false;
    for (let c = 0; c < 3; c++) {
      const d = Math.abs(a.data[i + c] - b.data[i + c]);
      total += d;
      max = Math.max(max, d);
      any ||= d > 0;
    }
    changed += Number(any);
  }
  return {
    changedPercent: (changed / (a.width * a.height)) * 100,
    mae: total / (a.width * a.height * 3),
    max,
  };
}
for (const [name, engine, device] of [...mobile, ...desktop]) {
  const browser = await engine.launch({
    headless: true,
    ...(engine === chromium
      ? {
          channel: "chromium",
          args: process.platform === "darwin" ? ["--use-angle=metal"] : [],
        }
      : {}),
  });
  try {
    for (const colorScheme of ["light", "dark"]) {
      const page = await browser.newPage({ ...device, colorScheme });
      const errors = [],
        messages = [];
      page.on("pageerror", (error) => errors.push(error.message));
      page.on("console", (message) => {
        if (message.type() === "error") errors.push(message.text());
        else if (message.type() === "info") messages.push(message.text());
      });
      await ready(page);
      assert.equal((await state(page)).mode, "webgl", `${name}: no bottle`);
      const record = {
        name,
        colorScheme,
        version: browser.version(),
        phases: [],
        errors,
        messages,
      };
      for (const [phase, progress] of [
        ["start", 0],
        ["turn", 0.15],
        ["approach", 0.42],
        ["unwrap", 0.7],
        ["flat", 0.999],
        ["html", 1],
      ]) {
        await scroll(page, progress);
        const data = await state(page);
        assert.equal(data.mode, "webgl");
        assert.equal(data.fallback, undefined);
        if (phase === "html") assert.equal(data.phase, "html");
        await page.screenshot({
          path: join(output, `${name}-${colorScheme}-${phase}.png`),
        });
        record.phases.push({ phase, ...data });
      }
      record.layout = await page.evaluate(() => {
        const face = document
          .querySelector("[data-label-face]")
          .getBoundingClientRect();
        const contents = document
          .querySelector(".contents-line")
          .getBoundingClientRect();
        return {
          pageTop: document.querySelector("#label-page").getBoundingClientRect()
            .top,
          face: face.toJSON(),
          clipped: contents.bottom - face.bottom,
          overflow: document.documentElement.scrollWidth > innerWidth,
        };
      });
      assert(Math.abs(record.layout.pageTop) < 1);
      assert.equal(record.layout.overflow, false);
      assert(record.layout.clipped <= 1, "Label small print is clipped");
      assert(record.layout.face.width <= 780);
      record.diff = await difference(
        join(output, `${name}-${colorScheme}-flat.png`),
        join(output, `${name}-${colorScheme}-html.png`),
      );
      // Canvas and DOM antialias differently, especially with a capped GPU DPR.
      // This catches a wrong texture/layout/theme, not harmless edge coverage.
      assert(
        record.diff.mae < 5,
        `Handoff changed: ${JSON.stringify(record.diff)}`,
      );
      assert.deepEqual(errors, []);
      if (
        mobile.some(([mobileName]) => mobileName === name) &&
        colorScheme === "light"
      ) {
        await scroll(page, 0);
        record.cadence = await page.evaluate(async () => {
          const travel = document.querySelector("#label-page").offsetTop;
          const intervals = [];
          let previous;
          for (let i = 0; i < 160; i++) {
            const now = await new Promise((resolve) =>
              requestAnimationFrame(resolve),
            );
            if (i > 40) intervals.push(now - previous);
            previous = now;
            scrollTo(0, travel * (0.03 + (0.8 * i) / 160));
          }
          return {
            fps:
              (1000 * intervals.length) / intervals.reduce((a, b) => a + b, 0),
            p95: [...intervals].sort((a, b) => a - b)[
              Math.floor(intervals.length * 0.95)
            ],
            samples: intervals.length,
            over33: intervals.filter((n) => n > 33.4).length,
          };
        });
        assert.equal((await state(page)).mode, "webgl");
        await scroll(page, 0.5);
        const before = await page.evaluate(() => ({
          y: scrollY,
          travel: document.querySelector("#label-page").offsetTop,
        }));
        await page.setViewportSize({
          width: device.viewport.width,
          height: device.viewport.height + 120,
        });
        await settle(page);
        const after = await page.evaluate(() => ({
          y: scrollY,
          travel: document.querySelector("#label-page").offsetTop,
        }));
        assert.deepEqual(
          after,
          before,
          "Toolbar resize changed native scroll progress",
        );
        await scroll(page, 1);
        assert.equal((await state(page)).phase, "html");
        record.toolbar = { before, after };
      }
      results.push(record);
      await save();
      console.log(
        `${name} ${colorScheme}: WebGL → HTML; diff MAE ${record.diff.mae.toFixed(3)}`,
      );
      await page.close();
    }
    if (!mobile.some(([mobileName]) => mobileName === name)) continue;
    for (const scenario of [
      "two-cores",
      "no-half-float",
      "no-webgl",
      "reduced-motion",
      "save-data",
    ]) {
      const page = await browser.newPage({
        ...device,
        ...(scenario === "reduced-motion" ? { reducedMotion: "reduce" } : {}),
      });
      const messages = [];
      page.on("console", (message) => {
        if (message.type() === "info") messages.push(message.text());
      });
      await page.addInitScript((scenario) => {
        if (scenario === "two-cores") {
          Object.defineProperty(navigator, "hardwareConcurrency", { value: 2 });
          Object.defineProperty(navigator, "deviceMemory", { value: 2 });
        }
        if (scenario === "save-data")
          Object.defineProperty(navigator, "connection", {
            value: Object.assign(new EventTarget(), { saveData: true }),
          });
        if (scenario === "no-webgl") {
          const get = HTMLCanvasElement.prototype.getContext;
          HTMLCanvasElement.prototype.getContext = function (type, ...args) {
            return type.startsWith("webgl")
              ? null
              : get.call(this, type, ...args);
          };
        }
        if (scenario === "no-half-float") {
          const get = WebGL2RenderingContext.prototype.getExtension;
          WebGL2RenderingContext.prototype.getExtension = function (name) {
            return name.includes("color_buffer") ? null : get.call(this, name);
          };
        }
      }, scenario);
      await ready(page);
      const data = await state(page);
      if (["two-cores", "no-half-float"].includes(scenario)) {
        assert.equal(data.mode, "webgl");
        await scroll(page, 1);
        assert.equal((await state(page)).phase, "html");
      } else {
        assert.equal(data.fallback, scenario);
        assert.equal(
          messages.filter((m) => m.startsWith("[rush bottle] fallback:"))
            .length,
          1,
        );
      }
      results.push({ name, scenario, data, messages });
      await save();
      console.log(`${name} ${scenario}: ${data.mode}`);
      await page.close();
    }
  } finally {
    await browser.close();
  }
}
console.log(`Evidence: ${output}`);
