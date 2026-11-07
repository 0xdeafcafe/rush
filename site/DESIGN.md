# The rush website: design

The front page is the bottle and its label, enlarged into a page. The chosen
reference is the small yellow bottle in `concepts/label/inspo.webp`: saturated
stock, a red wraparound slogan, dense warning panel and regulatory small print.
The slogan “GET THEM ALL IN.”, cut-letter RUSH paths and product copy are rush's
own. No reference slogan, mascot or letter artwork is reused.

The original site's live agent list, floating question and landing chirps stay.
Their captured text shows the product; the label supplies the furniture.
`ANTI-SLOP.md` is the review bar, `BRIEF.md` and the README are the claim sources.

## Build and assets

Astro 7 builds static HTML. TSX components have no `client:` directives and ship
no React runtime. `Label.tsx` is the single source of the visible label. `Zine.tsx`
keeps the existing agent list, feed, live-view explanation and supporting evidence.
`index.astro` owns the bottle stage and first-screen selectable install command.

`three` is pinned to **0.180.0** in package.json and the lockfile. `bottle.ts`
dynamically imports it and RoomEnvironment after an observed first contentful
paint. Vite emits hashed, same-origin chunks. There is no CDN or import map.
`motion.ts` handles only the list and feed; no animation framework is shipped.

`src/assets/bottle-still.png` is exported from the real scene and passes through
`astro:assets` as PNG. Every terminal screen is raw ANSI captured by `tools/shots` and rendered as
text by `AnsiScreen.tsx` at build time. No terminal screenshots are shipped. `og/og.html`
uses the still on yellow and produces `public/og.png` through headless Chrome.
The original bottle favicon and apple-touch icon remain. The 404 uses the same
palette and typography. `.github/workflows/pages.yml` deploys only on main.

## Tokens

Tokens are in `src/styles/site.css` (terminal evidence) and `label.css` (the label).

| Token | Value and reason |
| --- | --- |
| `--yellow`, `--ground` | `#ffe600`, the saturated yellow label stock. Dark ground `#291508`, the bottle's amber. |
| `--red`, `--print` | `#c21e16`, dense printing red with 4.75:1 on yellow. Dark print `#ff412c`, 5.02:1 on amber. |
| `--ink` | `#21170a` for prose, 13.91:1 on yellow; dark `#ffe600`, 13.75:1 on amber. |
| `--band`, `--band-type` | Red `#c21e16` and yellow `#ffe600` in both themes. |
| Terminal colours | Captured SGR colours are authoritative, including dim and reverse. Default cells use foreground `#f0ece6` and background `#111111`; the parser supplies the standard 16/256 palette where indexed colours occur. These colours belong only to evidence. |
| Provider marks | Copilot `#b3a6f2`, Codex `#6cc7b0`, Gemini `#7ea6f0`, from the real captured screen. |
| `--sans` | Helvetica Neue, Helvetica, sans-serif: packaging grotesque available on rush's target Mac. Weights 400, 700 and 800. |
| `--condensed` | Impact, Arial Narrow, sans-serif: commercial label lettering. Band skew −12°, original SVG wordmark also leans 12°. |
| `--mono` | Menlo, Consolas, monospace: real terminal text, source ID and commands. No downloaded fonts. |
| Type | Small 14 px, body 18 px, lead 24 px, heading 36–56 px, band 64–128 px. Mobile body 16, lead 22, small print 12 and band 56. Warning 40/32. SVG scales independently. |
| Leading | Prose 1.6, warning caps 1.35, captions 1.45, headings 1.04. Terminal 1.2 at a container-relative 11–13 px. |
| Spacing | Label plate: 780 × 820, max-width 780, container-unit scaling; outside the plate, 8 px base: 16, 24, 32, 40, 48, 56, 64, 80. Sheet maximum 1440 px, gutter 24–80 px. Optical letter offsets and terminal cell crop coordinates are deliberate exceptions. |
| Rules | 1 px fine rules, 2 px divisions, 3 px warning and live-view box, 8 px feed and install bars. |
| Radius / shadow | Zero radius and no UI shadow. The physical bottle has a generated soft contact shadow. |
| `--ease`, `--move` | `cubic-bezier(.2,0,.2,1)`, 400 ms row movement after a 220 ms question beat. Chirps fade up 8 px over 250 ms. |

## The same label throughout

The printing plate is **780 × 820**, regardless of viewport shape. `.label-frame`
is centred, at most 780 px wide; container units scale the composition uniformly
on narrow screens. The surrounding yellow/amber landing fills at least one
`dvh`. Large or tall windows add stock around the label instead of shrinking
its contents onto the bottle. The band has one complete slogan; the trailing
“EVERY LAST AGENT.” no longer runs round an invisible side.

`label-canvas.ts` waits for `document.fonts.ready`, measures a hidden clone of
`Label.tsx` at the real label width, then paints directly to 2D canvas. DOM
Ranges supply line breaks and glyph positions, a baseline probe supplies the
font baseline, computed CSS supplies colours/borders/tracking, and Path2D draws
the existing SVG paths. A glyph-position path handles older Canvas engines
without `letterSpacing`. There is no foreignObject, SVG image decode or image
readback to taint WebKit's texture. This painter supports the label's actual
boxes, text and paths, not arbitrary future HTML/CSS.

The normal curved texture is 1560 × 1640 (including on large/tall screens).
A lower-detail tier uses 780 × 820. A second texture is measured from the same
layout at up to DPR 2 for the flat plane. The final frustum rectangle matches
the centred HTML frame's width, height and vertical offset. The last 12% of
travel holds that image; visibility swaps atomically at the end, without a
crossfade or page translation. Layout and glyph positions match; Canvas/GPU
and DOM antialiasing are not byte-identical, particularly on DPR 3 phones with
the requested GPU DPR cap. `MOBILE-REVIEW.md` records measured differences.

## Bottle and native scroll

The body and shoulder use a spline lathe: 64 profile samples / 64 angular
segments on mobile or modest reported hardware, 120 / 96 on larger devices.
The cap has a cylinder, two lips and 64 instanced ridges. Yellow plastic uses
MeshPhysicalMaterial, clearcoat 1, roughness .21 (cap .35), metalness .02, IOR 1.48.
The **label uses those same body profile samples and angular vertices**, bounded
from y = −1.46 near the base to y = .30 below the shoulder. Radius is the body's
piecewise interpolation × 1.002, with polygonOffset −1/−1. There is no separate
radius approximation or floating sleeve. Label roughness .32 / clearcoat .45
shares the room's reflections. Its lighting gradually becomes plain printing
ink during flattening, with an unlit shader branch at fully flat geometry.

RoomEnvironment uses PMREM at 128 px on mobile / 256 desktop. Half-float colour
buffer support is checked first; without it, direct lighting preserves the
bottle and unwrap. Label and shadow textures are ordinary unsigned-byte canvas
textures. WebGL2 requests only alpha and antialiasing, without a powerPreference
or failIfMajorPerformanceCaveat veto. ACES exposure .72, environment intensity
.65, white key 1.65, warm rim .8 and hemisphere .3 remain object lighting. The
256 px radial alpha texture is a soft contact shadow.

Native scroll has 2.5 stage heights of travel on desktop, 1.75 on mobile.
Travel is rounded to an integer CSS pixel so fractional mobile heights cannot
leave the final frame just short of the HTML swap. The stage follows `100dvh`
(with `svh` fallback); toolbar height changes re-fit the camera and HTML landing
without changing travel or calling scrollTo. Width changes/rotation preserve
progress and regenerate the measured texture. Touch/wheel events are untouched.

Shader compilation, texture uploads and two hidden renders precede reveal.
DPR is at most 2, initially 1.5 on mobile/modest hardware, then 1 if sustained
steady-state frames cost too much. Uploads, resize and a one-second/30-frame
warm-up do not count. Thirty expensive steady-state samples lower detail before
any fallback; the final exit needs over 90 expensive samples at the lowest tier
and a frame over 100 ms. This measures CPU submission cost, not GPU profiling.
Rendering is requested by scroll/resize/theme/visibility, never an idle loop.

The first screen's tap command remains plain selectable text beside the GNU
restricted-shell note. Skip to label focuses its HTML. Hidden HTML stays inert
until the swap. A reader who has already reached the label before loading
finishes keeps the static page; small early scrolls within the bottle no longer
cancel enhancement.

## Motion below the label

- The Agents list uses two real captures of the installed rush on fictional
  fixture records, before and after the tax question. At 55% of the viewport the question arrives and the tax-test row floats
  into Needs you. Counts change with it. Scrolling back near the bottom reverses
  it with hysteresis, avoiding flicker.
- The feed is captured from rush's actual dock. Its labelled example chirps land one
  per eighth of a viewport, oldest first, and reverse as the dock leaves.
- A live reduced-motion preference removes both motion states. Without JS the
  list and feed show their final states; evidence is never hidden by CSS alone.

## Fallbacks and limits

Reduced motion and Data Saver deliberately use the still. Reported memory ≤2 GB
or ≤2 hardware threads now select lower detail, never exclude a device. Missing
WebGL, import/texture failure, context loss or sustained bad performance after
quality reduction still leave a usable still and normal HTML. The visitor can
choose **Use still image**; live preference changes release GPU resources and
preserve the reading position.

Every fallback sets `#bottle-intro.dataset.fallback` and logs one
`console.info('[rush bottle] fallback: …')` per page. `dataset.failure` retains
an available exception message. `dataset.mode`, `quality`, `painter`, `texture`
and `reflections` identify the active rendering path. No-JS retains the command,
links and terminal evidence. Low-power detection remains a quality heuristic,
not a portable OS Low Power Mode API.

Playwright WebKit/iPhone 15 and Chromium/Pixel 8 emulation are checked in
`checks/bottle.mjs`, including both colour schemes, all requested desktop sizes,
the two-core case, missing half-float support, fallbacks, toolbar resize and
handoff differences. This is not a physical iPhone/Android GPU certification.

## Claims and review

The source batch is real commit `98cec507f9d103d7f016b20bdc2cb95dfdb37153`, the
concept's source reference, not a regulatory ID or an asserted release version.
The memory note uses the README's 72 MB observation after five hours and 300-odd
sessions, with separate ~7 MB hosts. The older 40 MB claim described Go and was
removed. Fictional projects, chirps, simulated deploy and the example PR number are labelled.

`SLOP-REVIEW.md` records the initial port; `MOBILE-REVIEW.md` supersedes its
bottle measurements with mobile checks, handoff image diffs, download
weight, observed frame cadence and remaining limitations. Browser measurements
are not Rust performance claims and do not imply phone measurements.

## Terminal text, never terminal pictures

`tools/shots -ansi-out DIR -ansi-only` exports tmux's exact frame and adjacent
JSON dimensions/version/time. Raw captures live in `src/screens/`, including the
two list states. Their README gives regeneration commands and fixture provenance.
The tool uses its own tmux server, an isolated home and inert worker stand-ins.

`src/lib/ansi.ts` parses SGR standard/bright 16-colour, 256-colour and truecolour
foreground/background, bold, dim, italic, underline and reverse. Dim blends only
foreground with background. OSC links and other controls never become markup or
links. React escapes every text span. Grapheme widths, including wide characters
and combining marks, survive cropping at terminal cell boundaries. Sixteen tests
cover encodings, attributes, control removal, crops and every capture's dimensions.

`AnsiScreen.tsx` emits `<pre>` lines and spans, never an image or client-side
renderer. Contiguous cells with identical attributes share a span. Each span has
its actual terminal width in `ch`; SFMono-Regular, Menlo and Consolas are the
chosen stack, with ligatures and kerning disabled. Terminal leading is 1.2.
Container-relative font size reduces from 13 px to an 11 px minimum; only then
is horizontal scrolling allowed, inside a labelled, keyboard-focusable region.
The whole document never pans sideways.

Screen chrome, metrics, borders and status lines use `user-select: none`. Actual
content ranges retain `user-select: text`, including command spaces, paths,
branches, conversation prose and chirps. Mobile preserves the same cell grid;
it does not wrap or silently hide columns. Product captions explain the captures.
