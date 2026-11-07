# The rush website: design

rush.forbes.red is one page set like an issue of a small magazine about one tool: big condensed type, an asymmetric grid, rush's own screens cropped tight, and pull-quotes from agent chirps. The cover and the back page are printed like the bottle's label; the pages between are white, or black in dark mode. `ANTI-SLOP.md` is the bar it's checked against; `BRIEF.md` is what it may say.

## How it's built

- Astro, static output. `src/pages/index.astro` renders `src/components/Zine.tsx` (and the TSX under it) to HTML at build time, with no `client:` directives, so no React reaches the browser.
- Screenshots live in `src/assets` and go through `getImage` from `astro:assets`, which makes a 1600 px and a full-size webp and the `srcset`. Only the ones on the page are copied in.
- One script, `src/scripts/motion.ts`, bundled by Astro: about 2 KB.
- `public/og.png` is rendered from `og/og.html` with headless Chrome (the command is at the top of that file). `public/favicon.png` and `apple-touch-icon.png` are `docs/brand/rush-icon.png`, resized with `sips`.
- `npm run build` runs `astro check` then `astro build`. `.github/workflows/pages.yml` builds and publishes `site/dist` on a push to main that touches `site/`.

## Tokens

All in `src/styles/site.css` `:root`.

| Token | Value | Why |
| --- | --- | --- |
| `--label` | `#ffe600` | The label's yellow, flat. The cover, the back page and the bottle's label. |
| `--red` | `#e8231f` | The label's RUSH red. The wordmark and the quote marks only: 4.4:1 on white, so display sizes only. |
| `--ink` | `#111010` | The cap's black. Type on the yellow, and the install block. |
| `--ground` / `--text` | `#fff` / `#111010`; dark `#121110` / `#f3f1ed` | White or black grounds, so the yellow and red carry the page. Dark follows the system. |
| `--muted` | `#55524e`; dark `#aaa59d` | Captions. 7.6:1 and 7.9:1 on their grounds. Leans toward the cap black, not slate. |
| `--screen`, `--t-*` | `#111110`, text `#e2ddd3`, sub `#a39d93`, faint `#6f6a62`, rule `#3a3632`, orange `#d97757`, gold `#e5b567`, green `#7fbf8a`, blue `#8fb3d9` | rush's own terminal colours, taken from the agents capture. They appear only inside rush's screens. |
| `--sans` | Helvetica Neue | rush only runs on Macs, and every Mac has it. Condensed Black (`font-stretch: condensed; font-weight: 900`) for display, echoing the label's squashed RUSH; 400 and 700 for text. Arial off a Mac. |
| `--mono` | `ui-monospace`, SF Mono, Menlo | rush's screens and commands, as a Mac terminal shows them. Never at heading size. |
| Sizes | wordmark `clamp(7rem, 31vw, 28rem)`, `--display` `clamp(2.75rem, 6.4vw, 6rem)`, `--lead` `clamp(1.25rem, 1.9vw, 1.625rem)`, `--body` 18 px at 1.6, `--small` 15 px | Five sizes. Terminal text is 13 px (11.5 px narrow) on a 20 px (18 px) row, like a terminal. |
| `--gap` | `clamp(1.25rem, 4vw, 4rem)` | The grid's one gutter; sections pad `clamp(3.5rem, 8vw, 7.5rem)`. |
| Radius | none | Labels and printed pages are square. |
| Shadow | none | The bottle's glass shading is part of the drawing, not a UI shadow. |
| `--ease`, `--move` | `cubic-bezier(.2, 0, .2, 1)`, 400 ms | The one easing. A row floating up takes 400 ms after a 220 ms beat; a chirp lands in 250 ms. |

## Layout

- Max width 1440 px. Two-column grids are uneven (7/4, 6/5, 5/6, 3/5) and change per section, so no two sections have the same shape.
- The agent list bleeds off the right edge; the subagents shot sits at 78% width; the feed dock is pushed right under the pull-quote.
- Screenshots are cropped to one rectangle of the 2x original (`Shot.tsx`: `x`, `y`, `w`, `h` in source pixels, plus a narrower crop under 760 px), never scaled past native size, and start below the macOS window chrome.

## Motion

All of it is tied to where the page is scrolled, reversible (scroll back and it goes back), and never takes the scroll over. `<html>` gets `.motion` before first paint unless `prefers-reduced-motion` asks for less; without `.motion` (or without JS) every part shows its final state, still.

- **The bottle** (cover): the drawn bottle from `concepts/bottle`, its label cut into 72 strips round a cylinder. It makes one full turn while the cover scrolls away, swaying ±6° about its resting −12° tilt. Rendered facing front at build time.
- **The list** ("Whoever needs you goes to the top"): rush's Agents screen as text, from `docs/screenshots/agents.webp`. Until it's 55% of the way up the screen, it shows the tax test working ("reading the rounding tests", made up). Then its question arrives ("asks: Round half up, or half to even?"), the counts in the header and group titles change, and after a 220 ms beat the row and its worktree line float up into Needs you over 400 ms while the rows they pass move down. It goes back only once the list is down near the bottom again, so it never flickers.
- **The feed** ("They talk amongst themselves"): the feed dock as rush draws it (`crates/rush-ui-social/src/dock.rs`). Its three example chirps land one at a time, one per eighth of the screen the dock rises, each fading up 8 px in 250 ms.

## Copy

British English, short, specific. Every claim traces to the README or `BRIEF.md`. Anything invented (the chirps, the tax test's line before its question, the list's projects) is labelled as an example where it appears.
