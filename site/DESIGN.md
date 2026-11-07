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
| Spacing | 8 px base: 16, 24, 32, 40, 48, 56, 64, 80. Sheet maximum 1440 px, gutter 24–80 px. Optical letter offsets and terminal cell crop coordinates are deliberate exceptions. |
| Rules | 1 px fine rules, 2 px divisions, 3 px warning and live-view box, 8 px feed and install bars. |
| Radius / shadow | Zero radius and no UI shadow. The physical bottle has a generated soft contact shadow. |
| `--ease`, `--move` | `cubic-bezier(.2,0,.2,1)`, 400 ms row movement after a 220 ms question beat. Chirps fade up 8 px over 250 ms. |

## The same label throughout

On load, `bottle.ts` serialises `Label.tsx`'s rendered DOM into an SVG
foreignObject, including the already-loaded same-origin CSS and resolved theme
tokens. It waits for fonts. Rasterisation is at the current viewport dimensions;
there is no second canvas layout, hard-coded font positions or bottle-only copy.
The resulting 2–4k texture is used from the first curved frame through the unwrap.
A native-pixel resolution level from the same SVG avoids text resampling in the
last flat frames. The final plane uses an unlit, untone-mapped material so colours
match CSS exactly. No downloaded texture or HDRI is needed.

A 160 × 60 subdivision mesh moves from a curve following the bottle profile to
a plane exactly filling the camera frustum. The last 12% of travel holds that
flat plane. At the end, WebGL visibility and HTML visibility swap in one frame.
There is **no crossfade, opacity ramp, text replacement or page translation**.
The actual HTML begins at that same scroll coordinate. The opening label is at
least one viewport tall, so later sections cannot leak into its rasterisation.
Resize remeasures the frustum, regenerates the texture and preserves progress.
Theme changes regenerate the same label with the current resolved tokens.

## Bottle and native scroll

The squat body and shoulder use a spline lathe with 128 angular segments. The
cap has a cylinder, two lips and 64 instanced ridges. Yellow plastic uses
MeshPhysicalMaterial, clearcoat 1, roughness .21 (cap .35), metalness .02, IOR 1.48.
RoomEnvironment is PMREM filtered for reflections. ACES exposure .72, environment
intensity .65, white key 1.65, warm rim .8 and hemisphere .3 are object lighting,
not extra UI colours. A generated 256 px radial alpha texture supplies contact.

A sticky stage has 2.5 heights of travel on desktop, 1.75 on mobile. Scroll turns
the bottle, dollies in from .18–.64 and unwraps from .54–.88. Native wheel, touch,
keyboard and anchors retain their normal behaviour. Rendering is requested only
by scroll, resize, theme or visibility changes, never an idle animation loop.
DPR is capped at 1.75 desktop and 1.5 mobile while curved. The flat plane uses
the actual display DPR so Retina text matches the DOM. Rendering stops below
the opening.

The first screen contains `brew install 0xdeafcafe/tap/rush`, selectable plain
text beneath “Every coding agent. Under one cap.” The GNU restricted-shell warning
is alongside it and repeated in the label warning. Skip to label focuses the HTML.
Hidden HTML is inert until the swap, then becomes normal selectable, accessible DOM.

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

Reduced motion, Data Saver, reported memory ≤2 GB, ≤2 hardware threads, failed
WebGL, major rendering caveats, import/texture failures and context loss use the
still and normal-flow HTML. Sustained expensive CPU frames also exit the scene.
There is no portable OS Low Power Mode API; capability detection is a heuristic.
The visitor can choose **Use still image** at any time during the opening.
A live preference change releases GPU resources and preserves the reading position.

The still and all copy are useful before three.js loads. If someone scrolls or
follows an anchor before loading completes, enhancement does not take over.
No-JS retains the selectable command, links and all product evidence.

## Claims and review

The source batch is real commit `98cec507f9d103d7f016b20bdc2cb95dfdb37153`, the
concept's source reference, not a regulatory ID or an asserted release version.
The memory note uses the README's 72 MB observation after five hours and 300-odd
sessions, with separate ~7 MB hosts. The older 40 MB claim described Go and was
removed. Fictional projects, chirps, simulated deploy and the example PR number are labelled.

`SLOP-REVIEW.md` records the built-site screenshots, handoff image diffs, download
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
