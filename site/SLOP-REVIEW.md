# Bottle front-page review

Reviewed 2026-11-07 against `ANTI-SLOP.md`, the brief and the README, using the
production build served by Python's HTTP server. Chrome 156.0.8078.80, headless
with ANGLE Metal on Apple M1 Max. The 390 px checks use an iframe inside the
1440 × 900 browser; separate device-scale checks cover Retina rasterisation.

## What shipped

The chosen concept becomes the opening: squat physical bottle, native scroll
into its label, curved mesh unwrapping, then ordinary HTML. The red band,
warning panel, cut-letter wordmark and slashes belong to rush. The reference's
slogan, mascot and exact lettering are absent. The original zine's question
moving into Needs you and its landing chirps remain below the label.

`Label.tsx` supplies both the HTML and the DOM rasterised into the texture.
CSS, fonts, layout, theme and viewport measurements are shared. The flat plane
uses native device pixels and swaps visibility with HTML atomically; there is
no fade, substituted typography or translated page. The hero install command
is plain selectable text, with the GNU restricted-shell warning nearby.

Every product screen is selectable text from real tmux ANSI captures of the
installed `rush 0.1.0-beta.9 (804369ce9ba0)`. The two agent-list states are
separate captures. Fictional repositories, fixture workers, chirps and simulated
deployment are disclosed; these are actual rush rendering, not real customer
work or claims that a deployment ran. `src/screens/README.md` records provenance
and regeneration. All terminal bitmap imports and assets were removed from
the site; `docs/screenshots` was left alone. The one page image is the bottle.

## Visual and functional checks

- 1440 × 900 and 390 × 844, light and dark: opening, curved/unwrap frame,
  last flat frame, HTML handoff, question movement and all three chirps.
  No document overflow or browser exceptions. HTML begins at viewport y = 0
  at the handoff. Also checked 320 × 480 and resize to 768 × 1024.
- Reduced motion at load and changed live, no JavaScript, disabled WebGL,
  forced context loss, blocked three.js import and Data Saver: usable still
  and normal HTML. Data Saver requested neither optional three.js module.
  Skip to label focuses the label and releases its inert state.
- Both real list captures inspected; the tax-test row moves from Working to
  Needs you and its captured question appears. The feed's captured rows land
  in order. Reduced motion and no JS show the final states immediately.
- List, feed, subagent and command screens checked separately at desktop and
  mobile. Chrome reports Menlo-Regular as the actual terminal font. Box corners
  and rules join, including at an enlarged 26 px inspection size. Cell widths
  remain intact; text never wraps into neighbouring columns.
- Commands, paths, branch names and chirps use selectable spans. Chrome,
  columns, borders and status decoration do not select. Command spaces survive
  selection. Mobile shrinks to the 11 px minimum, then permits scrolling only
  inside each focusable terminal region.
- The 1280 × 640 social card was rendered from `og/og.html` with headless
  Chrome and inspected. Favicon and metadata retained/updated; dark 404 checked.
  The card is 116,608 bytes and is not part of the ordinary page download.

## Handoff image differences

Screenshots were taken at progress .999 (flat WebGL) and 1.000 (HTML), at the
same scroll-aligned viewport. Pixel comparison used RGB absolute differences.
MAE below is on the 0–255 channel scale, not a normalised percentage.

| Viewport | Differing pixels | Mean absolute error |
| --- | ---: | ---: |
| 1440 × 900, DPR 1, light | 8 / 1,296,000 (0.000617%) | 0.0000787 |
| 1440 × 900, DPR 1, dark | 8 / 1,296,000 (0.000617%) | 0.0000826 |
| 390 × 844, DPR 1, light | 0 | 0 |
| 390 × 844, DPR 1, dark | 0 | 0 |
| 1440 × 900, DPR 2, light | 0.193885% | 0.0007360 |
| 390 × 844, DPR 3, light | 0.206755% | 0.0007187 |

Retina differences are overwhelmingly one-level antialiasing/quantisation
changes. Only two desktop Retina pixels and one mobile Retina pixel differ by
more than ten channel levels; maximum difference is 50. There is no measurable
layout shift or font-size change. This is visually continuous, but the desktop
and Retina images are **not byte-identical**. Native-DPR flat rendering fixed
the visibly softer handoff caused by retaining the curved scene's DPR cap.

## Weight and frame cadence

Reproducible budget: gzip for HTML/CSS/JS, original encoded bytes for PNG.
The local Python server was not compressing responses; these are compressed
asset sizes, not a claim about its measured wire transfer. Production Brotli,
headers and cache state will change transfer sizes.

| Resource | Bytes |
| --- | ---: |
| HTML, including every terminal span | 8,746 |
| CSS | 4,205 |
| Entry/motion/bottle script | 5,448 |
| Optimised bottle still | 66,817 |
| Favicon | 6,872 |
| Optional three.js | 174,480 |
| Optional RoomEnvironment | 925 |
| **Complete page** | **267,493 (261.2 KiB)** |
| **Before optional 3D modules** | **92,088 (89.9 KiB)** |

No downloaded HDRI, model, font or label texture. The 2–4k label and contact
shadow are generated locally and still cost GPU memory. Raw committed ANSI is
76,692 bytes; it is consumed at build time, not downloaded separately. Astro
emits an unused React client chunk; this page neither references nor loads it.
Vite warns that the three.js chunk is above 500 kB uncompressed (694,905 bytes).
It remains a deferred optional chunk, 174,480 bytes gzipped.

Observed first contentful paint was 292 ms; the first three.js request started
at 309.1 ms, RoomEnvironment at 331.8 ms. These are local-server observations,
not a network benchmark. After twelve warm-up frames, 138 scrolling rAF
intervals per case measured:

| Case | Observed cadence | p95 interval | Intervals >33.4 ms |
| --- | ---: | ---: | ---: |
| Desktop DPR 1 | 60.0 fps | 16.7 ms | 0 |
| Desktop DPR 2 | 60.0 fps | 16.7 ms | 0 |
| Emulated mobile DPR 3 | 60.0 fps | 16.8 ms | 0 |

These are Chrome animation-frame cadence observations on M1 Max, not GPU
profiling or physical-phone results. Rendering is event-driven; the bottle
does not run an idle loop and stops below the opening. Curved rendering caps
DPR at 1.75 desktop / 1.5 mobile; flat frames use actual display DPR.

## Checklist judgement and deliberate exceptions

The dominant saturated yellow, red printing, tight warning caps and oversized
italic band come from the packaging reference. Helvetica and Impact are
deliberate packaging choices; Menlo is the actual terminal face here. No font
downloads, gradients in type, glowing hero backdrop, pill controls, fake
window chrome, icon-card grid, stock section sequence or generic testimonials.
The page retains unequal section weights, an original slash mark and the
product's real dense text. Tokens and motion are recorded in `DESIGN.md`.

The requested bottle is a brand object, not an abstract 3D stand-in for product
evidence; the real text evidence follows it. Lightning-like slashes and warning
caps are packaging language, not generic AI icons or repeated eyebrows. There
are precisely three motion ideas: bottle/label, question row, chirps. Native
scroll is untouched and no animation library is added.

Label/prose contrast is documented in DESIGN. Real terminal dim text is kept
faithful to the captured palette and does not universally meet 4.5:1; terminal
font size reaches 11 px to preserve its wide cell grid. Those are explicit
evidence exceptions, not a claim of full accessibility conformance. Prose and
controls use the page's larger type and visible focus treatment.

The scanner found no matches in built HTML/CSS/JS for the checklist's stock
utility/effect strings. Claims use the README/brief: the old Go-era 40 MB claim
was replaced by the qualified 72 MB observation. Batch text links a real
source commit rather than posing as a regulatory identifier. Examples are
labelled and the install route is the actual tap command.

`npm run build`: zero Astro errors, warnings or hints. `npm test`: 16 passing
ANSI tests covering encodings, attributes, controls, Unicode cropping and all
five capture dimensions. No Rust source changed or Rust build was run locally.

## Remaining limits

Safari, Firefox and physical phones have not been measured. The SVG
foreignObject texture path and OS font rendering deserve a real-device pass;
failure leaves usable HTML and the still. Low-power detection is a capability
heuristic, since browsers do not expose a portable OS Low Power Mode signal.
Mobile terminal evidence deliberately needs local horizontal scrolling once
it reaches its readable minimum. Tiny raster antialiasing differences remain
at the hard handoff as quantified above.
