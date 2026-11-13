# Mobile bottle and printed-label fixes

Checked 2026-11-07, based on main `6607b565`. This supersedes the original
bottle measurements in SLOP-REVIEW.md. Terminal text and product claims remain.

## What failed, and what was reproduced

The old two-core/two-GB check deterministically selected `low-power` in both
WebKit and Chromium. With those same mocked hardware hints, the fix completes
the bottle, approach, unwrap and HTML handoff. Hardware now chooses detail.

Unmodified macOS Playwright WebKit and Chromium both rendered the old
foreignObject texture. They did **not** reproduce the owner's exact phone
failure, and a macOS WebKit process is not an iPhone GPU. The old page had no
fallback log, so its precise on-device trigger cannot be established
retrospectively. The foreignObject/image-decode/readback route is now entirely
removed: the replacement paints paths, boxes and text to an origin-clean canvas
using the actual HTML's measurements, computed styles and loaded fonts.

The context requests only alpha/antialiasing. Missing half-float colour buffers
selects direct lighting, not a still. Shader compilation, both texture uploads
and hidden renders precede reveal. Upload/resize/warm-up costs do not count as
steady state, and quality drops before a sustained frame-cost fallback. Every
fallback records `intro.dataset.fallback` and one console.info message; an
exception, where available, remains in `dataset.failure`.

## Before and after the visual bugs

**Floating sleeve:** the old label used a separate radius approximation, a
large offset and an unlit material. The red band could project past the
shoulder/silhouette. Now its vertices use the body's exact sampled profile and
angular vertices, with radius × 1.002 and polygonOffset −1/−1. It extends from
y = −1.46 near the base to .30 below the shoulder. Physical paper material
shares the room's reflections with less clearcoat than the bottle. Opening,
turned and approached frames were inspected up close. No visible air gap.

**Viewport-shaped artwork:** previously a 1920-wide or 2446-tall viewport became
the texture, squeezing different layouts onto the same physical bottle. Now
there is one 780 × 820 printing plate, centred at max-width 780 in HTML.
Container units scale that composition on mobile. Normal curved textures are
1560 × 1640 at every tested viewport. RUSH and the warning share the front, one
complete slogan fits the band, and the contents line reaches the lower body.
Yellow/amber surrounds the label on wider/taller pages. The still PNG and
social card were regenerated from the corrected bottle.

## Browser matrix

Playwright **1.55.1**, WebKit **26.0**, Chromium **140.0.7339.186**, Apple M1 Max.
iPhone 15 uses the supplied descriptor: 393 × 659 viewport, 393 × 852 screen,
DPR 3. This Playwright version has no Pixel 8 registry entry. The test explicitly
sets a 412 × 839 viewport, 412 × 915 screen, DPR 2.625 and touch/mobile flags,
and adapts the registry's Android Chrome UA. It does not silently use Pixel 7
screen measurements. Emulation is not physical-device certification.

| Case | Result |
| --- | --- |
| iPhone 15 / WebKit, light and dark | WebGL → turn → approach → unwrap → HTML; no fallback |
| Pixel 8 settings / Chromium, light and dark | WebGL → turn → approach → unwrap → HTML; no fallback |
| 1440 × 900, 1920 × 1080, 2560 × 1440 | Both themes complete; centred 780 px label |
| 1576 × 2446 tall window | Both themes complete; same composition and texture dimensions |
| 390 × 844 | Both themes complete; intact fixed-ratio label |
| Two cores and 2 GB reported | Full WebGL path at lower initial detail, both engines |
| Half-float colour-buffer extensions absent | Full WebGL path with direct lighting, both engines |
| WebGL absent | `no-webgl`, one console.info, still and usable HTML |
| Reduced motion | `reduced-motion`, one console.info, still and usable HTML |
| Data Saver | `save-data`, one console.info, still and usable HTML |

Each normal case screenshots six scroll points. Tests assert no horizontal
document overflow, no clipped contents line, max-width 780, HTML top at zero,
no browser errors and no unexpected fallback. A +120 px toolbar-height test
kept scroll/travel unchanged: iPhone y 576 / travel 1153; Pixel y 734 / travel
1468. The dvh stage and camera changed height. Separate rotation checks reached
HTML at y = 0 without overflow. Live reduced motion and no-JS were inspected.
Generic-font substitution and missing Canvas letterSpacing also retained the
full path and unclipped label.

## Handoff and performance limits

The visibility swap is atomic, without a fade. The flat plane fits the measured
centred HTML rectangle. Canvas font rasterisation and capped DPR are **not
byte-identical** to native DOM text: phones sharpen slightly at the swap. The
screenshots show the same composition, wording and positions; this is not a
claim that every pixel is identical.

RGB mean absolute error on the 0–255 scale, flat WebGL versus HTML:

| View | Light | Dark |
| --- | ---: | ---: |
| iPhone 15 DPR 3 | 2.844 | 2.946 |
| Pixel 8 DPR 2.625 | 1.633 | 1.757 |
| 1440 × 900 DPR 1 | 0.666 | 0.820 |
| 1920 × 1080 DPR 1 | 0.417 | 0.512 |
| 2560 × 1440 DPR 1 | 0.234 | 0.288 |
| 1576 × 2446 DPR 1 | 0.224 | 0.276 |
| 390 × 844 DPR 1 | 1.035 | 1.331 |

After 40 warm-up intervals, 119 scrolling rAF intervals on this Mac measured
60.0 fps in WebKit (p95 21 ms) and 123.6 fps in Chromium (p95 8.8 ms). Neither
had an interval above 33.4 ms. These are emulated-device animation-frame cadence
observations, **not physical phone FPS or GPU timings**. Both retained WebGL.
Mobile curved frames use DPR 1.5, flat frames at most 2; the lowest tier uses
DPR 1 and a smaller texture.

The complete page budget is approximately **293 KiB**, with gzip HTML/CSS/JS
and encoded PNG sizes, including terminal text and favicon. Of this, 175,405
bytes are optional three.js/RoomEnvironment. No font, HDRI, model or label
texture downloads. Browser-check dependencies are dev only. Python's server
does not gzip responses; this is an asset budget, not its wire transfer size.

`npm run build`: zero Astro errors, warnings or hints. The existing Vite warning
about the optional three.js chunk exceeding 500 kB raw remains. `npm test`:
16 passing ANSI tests. `npm audit`: zero vulnerabilities. The anti-slop pass
retains physical packaging and real terminal text. The owner-requested fixed
plate deliberately scales small print below ordinary prose size on phones;
normal page prose stays larger and pinch zoom is available. No fake terminal
chrome or new stock effects.

## Reproduce

From the repository root, build and serve the site in a separate terminal.
Browser downloads and screenshots stay under TMPDIR.

```sh
npm --prefix site ci
npm --prefix site run build
```

```sh
python3 -m http.server 8768 --bind 127.0.0.1 --directory site/dist
```

```sh
PLAYWRIGHT_BROWSERS_PATH="$TMPDIR/playwright-browsers" site/node_modules/.bin/playwright install webkit chromium --no-shell
PLAYWRIGHT_BROWSERS_PATH="$TMPDIR/playwright-browsers" npm --prefix site run test:bottle
```

`$TMPDIR/rush-mobile-check/results.json` records paths, fallback reasons,
versions, cadence, layout and diffs. PNGs beside it record all six scroll points
for each size/theme. `BOTTLE_URL` overrides the URL. Remaining real-device work
is Safari on the owner's phone and Android hardware: WebKit emulation cannot
reproduce every iOS GPU/OS condition.
