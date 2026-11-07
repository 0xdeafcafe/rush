# Anti-slop checklist for the rush website

Carried over from the Sounding site, then extended on 2026-11-07 from a second pass over current write-ups, HN threads, Anthropic's own frontend-design guidance and large audits (1,590 Show HN pages, 120 builder-made pages). Sources are at the bottom. No single item damns a page: it's the stack of defaults together (commenters put the "this is slop" feeling at about 50 small tells). The page must fail almost none of these.

Key lesson of the 2026 pass: the first slop look (purple gradient, Inter, glass) has been replaced by a second, "tasteful" one (cream, serif, terracotta or sage, mono labels, 01/02/03). Anti-slop prompts produce a new cluster of defaults. A tell is an unchosen default, not a banned value: every colour, font and radius on the page must trace to a decision in `DESIGN.md`.

## Type
- [ ] No unchosen default fonts: Inter, Geist, Roboto, Arial, Open Sans, Lato or the system stack used only because they were the default.
- [ ] No "tasteful free" fonts used by reflex either: Space Grotesk (the model's favourite once Inter is banned), Instrument Serif, Fraunces, DM Sans, Cabinet Grotesk, Rajdhani, Share Tech Mono.
- [ ] No Instrument Serif (or any serif) headline paired with a mono caption, the 2026 "tasteful AI" default.
- [ ] No serif headline with one italic, accent-coloured word ("Software that *feels* alive").
- [ ] No small uppercase, tracked eyebrow or mono caption above every heading ("01 / GET INTO IT", "AN INDEPENDENT APP", "FEATURES").
- [ ] No all-caps, wide-tracked, mono section labels used as the page's chrome (about 42% of builder-made pages).
- [ ] No monospace face at H1 size (JetBrains Mono, Geist Mono, IBM Plex Mono) as the only way to say "developer".
- [ ] Not lowercase-everything as an affectation: headings and buttons in plain sentence case unless the brand voice truly is lowercase.
- [ ] No very light weights (200 to 300) on grey text; weights limited to a few (for example 400, 500, 600).
- [ ] One deliberate type scale with real contrast: 5 or 6 sizes at most, body at least 16 px, line height about 1.6 for body, tight tracking only on big display sizes.
- [ ] Prose measure no wider than about 65ch, and no full-container-width paragraphs.
- [ ] A font declared in CSS must actually load (no silent fallback to system sans).
- [ ] No gradient text (`bg-clip-text text-transparent`) on "the one word that matters", and no one-word accent colour in the headline.
- [ ] No Title Case On Every Heading, button and bullet.
- [ ] No em-dash-heavy copy, and no decorative middle dots or `//` as separators in headers ("Home // Work", "A · B · C" meta strings).

## Colour
- [ ] No purple, violet, indigo or blue gradients, and no Tailwind `indigo-*`, `violet-*`, `#6366f1`, or `--primary` hue 255 to 280.
- [ ] No "tasteful default" cluster: warm cream or parchment ground (`#faf8f5`, `#f4f1ea`, `bg-stone-50`) plus serif plus terracotta (`#d97757` and neighbours), sage or forest green. Any two together is a strong signal.
- [ ] No near-black ground with a single acid-green, vermilion or neon accent as the "dev tool" look (Linear-style `#08090a` canvas with `#23252a` hairlines).
- [ ] No glowing blob, aurora, mesh gradient or radial bloom behind the hero or behind the product frame, and no coloured `shadow-[0_0_*]` glows.
- [ ] No untouched shadcn palette: `components.json` not left on `baseColor: "slate"` with unedited CSS variables, no `rounded-lg border bg-card shadow-sm` on everything, no slate or zinc greys by default.
- [ ] One dominant colour, one accent (the label red or yellow) and one neutral. No palette-cycling cards (red, blue, green, orange), and no emerald or teal fallback.
- [ ] No permanent dark mode by reflex. Follow the system, with a real light design.
- [ ] Grey tints chosen to match the palette, not default grey (also tint shadows, not pure black or grey).
- [ ] Contrast of at least 4.5:1 everywhere, especially muted text on dark grounds.
- [ ] Not a flat, empty ground: if a texture is used it has a reason (see Layout).

## Layout and components
- [ ] No pill or badge above the hero headline ("✨ New", "AI-powered", "v1.0 is here", "Not on the App Store"), and no pulsing or blinking status dot in a corner.
- [ ] Not a centred hero with a subhead and two buttons, plus a logo strip (about half of builder-made pages centre everything).
- [ ] Not the stock section order: hero, logos, three features, stats, testimonial, pricing, FAQ, CTA band, four-column footer. Order follows the story, not the template.
- [ ] No row of three (or four, or six) identical icon-topped feature cards, and no bento grid of uneven icon boxes under a "FEATURES" label.
- [ ] No icon inside a rounded square tile, no Lucide-only icon set, and no sparkle, lightning bolt or brain icon to signal "AI".
- [ ] No emoji as icons, bullets or sidebar items (rocket, bolt, lock, check, sparkles).
- [ ] No check-mark list under the hero buttons ("No credit card, free forever, cancel any time") and no check-mark feature lists in cards.
- [ ] No "1 · 2 · 3" numbered steps, no decorative "01 / 02 / 03" section numbering, no stats row, and no A/B/C lettered lists.
- [ ] No stat banner with unsourced round numbers ("10M+", "99.9% uptime", "500x faster", "24/7") and no count-up animation.
- [ ] No card around every block of content, no nested cards, and no coloured left or top border strip (3 to 4 px) on cards. Group with whitespace, proximity and type instead.
- [ ] No glassmorphism or backdrop-blur sticky nav by reflex, and not the same `rounded-2xl`, `shadow-lg`, `p-6` on everything.
- [ ] No "glowing" or gradient hairline borders (1 px light border with a gradient or a top radial glow on cards).
- [ ] No fake window chrome: traffic-light dots (red, yellow, green), a fake title bar, a fake URL bar or a vertical rotated label.
- [ ] No grid or dot-pattern background, noise or grain overlay, or "blueprint" crosshair marks used as a default finish. If texture exists it is part of the identity (the label, the paper), not an overlay.
- [ ] No marquee or infinite-scroll strip of logos, tags or quotes, and no "Trusted by" row of logos that are not real.
- [ ] No testimonial carousel, and no testimonials with placeholder avatars, "Jane Doe" names or round-number praise.
- [ ] No three-tier pricing with a ringed "Popular" middle card, unless there really is pricing (rush may have none).
- [ ] No giant footer link grid duplicating the nav, with the same links in every column.
- [ ] No decorative captions ("A GOOD CONNECTION.", "Design preview"), badges, page counts or extra stats nobody asked for.
- [ ] Consistency: one corner radius (2 or 3 radii in total, and not `rounded-2xl` or `rounded-3xl` or `9999px` pills everywhere), one shadow rule, one spacing scale (8 pt), and no uniform section padding that makes every section the same weight.
- [ ] Section rhythm varies: not every section the same heading, subhead, then grid structure.
- [ ] No full-width content stretched to a wide container (centred `max-w-7xl` 12-column grid with everything the same width); use a narrower measure and let some things break the grid.
- [ ] Focus states are visible, alt text exists, favicon, title and meta description are real and differ per page.
- [ ] Nothing overflows or overlaps: text fits its buttons and containers at every width, edges align, icons and arrows are optically centred.

## Dev-tool pages
- [ ] No terminal-window mockup with traffic-light dots wrapped around invented output. Real rush screens, real captured text, or no mock at all.
- [ ] No fake or "perfect" demo output: round numbers, tidy timings, a `$` prompt in front of a command the tool does not take, or commands that were not run.
- [ ] No typewriter animation typing the terminal output or the headline letter by letter.
- [ ] No hero install line styled as a pill with a copy button and a `npx` or `curl | sh` that is not the real install route.
- [ ] No GitHub stars badge, "Open source" badge, or version pill used as decoration, and nothing that implies stats we do not have.
- [ ] No row of `kbd` keycap chips as decoration ("⌘K", "Ctrl+Enter") unless each is a real rush key shown in context.
- [ ] No "Built for developers", "Built for teams", "Built for speed" or "The AI-powered X for modern Y" mad-lib headline. Say what rush does for whom, in plain words.
- [ ] No "Linear clone" look: near-black canvas, hairline borders, radial glow over a dashboard frame, gradient headline word, glass cards.
- [ ] No abstract "dashboard" or 3D blob as the hero image in place of the actual product.
- [ ] No product name in lowercase mono with a blinking cursor as the logo as the only identity.
- [ ] No generic "Get started" or "Learn more" CTA, and no "→" arrow glued to every link and button; labels say the exact action ("Install rush", "Read the config docs").
- [ ] No Aceternity or Magic UI stock effects (spotlight, beams, meteors, shimmer buttons, animated borders) on a page that is not about them.
- [ ] No privacy or "runs locally" reassurance that was not asked for or cannot be backed by a link to the code.
- [ ] No leftover artefacts: "Vite + React" titles, default platform subdomains, builder badges, stale model names, an old footer year, section-label HTML comments, `Hero`/`Testimonials` class names, old product names in copy.
- [ ] The page names its actual agents, keys, files and commands (real names from the Rust port) rather than category words ("workflow", "agents", "automation").

## Motion
- [ ] No fade-and-slide-up on every section (`data-aos="fade-up"`, identical staggered reveals), and no motion library reveals as the only motion.
- [ ] No `whileHover={{ scale: 1.05 }}` or lift-with-shadow on every card, especially cards that are not clickable.
- [ ] No bounce or spring easing for its own sake, no count-up numbers, no auto-typing headlines, no glowing blob that follows the cursor.
- [ ] Motion must mean something: a row floating up, a chirp landing, the bottle. Two or three animation types at most, used consistently.
- [ ] No scroll-jacking, parallax or carousels, and nothing that alters native scroll speed.
- [ ] No animation that restarts when the cursor moves, and `prefers-reduced-motion` is honoured.
- [ ] Most interactions 150 to 300 ms; none over about 500 ms that feel like lag.
- [ ] No pointless hover effects, and no buttons that look clickable but go nowhere.
- [ ] No loading text that narrates the build ("Initialising workspace...") in place of a real state.

## Copy
- [ ] No adjective-stacked subtitles, "Elevate your...", "Supercharge", "Unleash", "Transform your", "Unlock the power of", "X, reimagined", "Say goodbye to X", or vague SaaS wording ("seamless", "powerful", "robust", "streamline", "cutting-edge", "effortless", "in just a few clicks").
- [ ] No "No X. No Y. Just Z." negation list, and no "Less X, more Y" or "Forget X. Focus on Y.".
- [ ] No "It's not X, it's Y" or "Not because X. Because Y." contrast reveal.
- [ ] No fragment drumbeat or tricolon headline ("Fast. Simple. Done.", "Build. Ship. Scale.", "Fast, secure and scalable"), and not three of anything by reflex.
- [ ] No rhetorical question then answer ("The result? Calm."), colon reveal ("The best part: ...") or "Here's the thing" setup.
- [ ] No wisdom-shaped aphorisms ("Speed is a feature.") or performed cleverness ("Spreadsheets with a pulse.").
- [ ] No slogan on every section (one wink per section at most) and no tidy closer ("The future of X starts now.").
- [ ] Headline says the result or the thing, not the technology, and could not sit unchanged on a competitor's site.
- [ ] No header, eyebrow, H1, description and footer all saying the same thing.
- [ ] No bold label on every bullet ("**Seamless integration:** ...") and no parallel bullet openers ("Ensures... Provides... Enables...").
- [ ] No synonym cycling: one name for one thing (agent, session, card) everywhere.
- [ ] No invented labels or false ranges ("from solo devs to enterprises"), no weasel sources ("trusted by industry leaders", "join thousands of developers"), no round-number claims.
- [ ] No generic FAQ filler or an FAQ with no answers. Answer only real questions.
- [ ] No narrated UI labels ("Choose a...") where a plain heading works, and no internal or backend terms leaking into user copy.
- [ ] Specific, product-true sentences in the owner's voice: British English, short, uneven in length.
- [ ] Real product evidence: real rush screens (as screenshots or as real text), not mock UI.
- [ ] Sentence and section rhythm is not uniform: some long, some short, some sections plainly larger than others.

## Craft
- [ ] Squint test: blur the page. It must not read as "dark hero, glow, one centred button, three cards".
- [ ] Side-by-side test: a stranger could tell it apart from a page generated by v0, Lovable, Bolt or Claude with a one-line prompt.
- [ ] Swap-the-logo test: replace "rush" with another dev tool's name and the page must stop making sense.
- [ ] Every colour, font, radius, shadow and easing is a recorded token in `DESIGN.md`, with the reason. Nothing is "whatever the model picked".
- [ ] Design came from a reference or mockup (the bottle, the label, a real terminal capture) first, and was then built. Not prompted from a blank page.
- [ ] Screenshot-and-compare loop done: built page checked against the intended design at mobile, tablet and desktop, differences fixed.
- [ ] No page drift: font, colour, radius, nav and footer identical across every page unless deliberately changed.
- [ ] The scanner pass: grep the output for `rounded-2xl`, `rounded-3xl`, `backdrop-blur`, `bg-clip-text`, `indigo-`, `violet-`, `shadow-lg`, `data-aos`, `whileHover`, `from-purple`, `#6366f1`, `lucide`, and explain each hit.
- [ ] Imperfect on purpose in a few places: something hand-set (a custom rule, a drawn mark, an odd crop) that a template would not do.
- [ ] Finish line done: favicon, social card, 404 page, per-page title and description, working links, real alt text, no placeholder content.

## Positive direction
Commit to one aesthetic and write its tokens in `DESIGN.md`: palette, type, radius, spacing and motion. For rush, that means:
- the bottle and its label as the signature, not a generic logo;
- the bottle's own loud colours, used flat and with confidence: label yellow, label red, cap black, amber glass. Not cream-and-terracotta, not salmon, not beige "paper": that is now the tasteful-AI default (see Colour);
- white or black grounds, so the yellow and red carry the page;
- the terminal itself as the product evidence: real screens from real sessions, set as real text in rush's real colours, with no window chrome and no invented output;
- lower case only where it is rush's real voice (the README's prose), never as a typographic effect on headings, nav and buttons;
- a clear type scale and lots of whitespace.

## Sources
- [Is your website AI slop? (DEV Community)](https://dev.to/laurilllll/is-your-website-ai-slop-how-to-tell-and-how-to-fix-it-1fn5)
- [AI Slop Fonts and Gradients (925studios)](https://www.925studios.co/blog/ai-slop-design-tells)
- [AI Slop Web Design guide (925studios)](https://www.925studios.co/blog/ai-slop-web-design-guide)
- [AI Slop Design fix guide (vibecodekit)](https://vibecodekit.dev/ai-slop-design)
- [AI slop website checklist (illos.dev)](https://illos.dev/guides/ai-slop-website-checklist)
- [AI Slop: the tells on a website (sailop)](https://sailop.com/blog/ai-slop-definitive-guide-2026)
- [20 Signs a Website Was Made by AI (SCALR)](https://scalr.sg/blog/20-signs-a-website-was-made-by-ai/)
- [Why every AI landing page looks generic (designdotmd)](https://freedesignmd.com/blog/why-ai-landing-page-looks-generic)
- [How to spot an AI-built website (falia)](https://falia.co/en/insights/web-design/spot-ai-built-website/)
- [AI Design Slop: 16 patterns (Developers Digest)](https://www.developersdigest.tech/blog/ai-design-slop-and-how-to-spot-it)
- [7 Signs a UI Has Been Vibe Coded (Fountain Institute)](https://www.thefountaininstitute.com/blog/signs-vibe-coded-ui)
- [How to avoid apps that look vibe coded (vibemole)](https://vibemole.com/resources/avoid-vibecoded-app-design)
- [Vibe coded website design: 8 mistakes (SiteCritic)](https://www.sitecritic.ai/blog/vibe-coded-website-design-8-mistakes-to-fix-now)
- [avoid-ai-design: audits AI frontend, names the "tasteful" defaults (GitHub, funboy322)](https://github.com/funboy322/avoid-ai-design)
- [Unslop UI Skill for Claude: Kill the AI Design Tells (claudecodehq)](https://www.claudecodehq.com/playbooks/unslop-ui)
- [The Anti-Slop Look Is Now AI Slop: 11 Claude Design Secrets (Medium)](https://medium.com/@anup.karanjkar08/the-anti-slop-look-is-now-ai-slop-11-claude-design-secrets-nobody-tells-you-0c11c2bb2359)
- [Prompting for frontend aesthetics (Anthropic cookbook)](https://platform.claude.com/cookbook/coding-prompting-for-frontend-aesthetics)
- [Improving frontend design through Skills (Anthropic)](https://claude.com/resources/articles/improving-frontend-design-through-skills)
- [Why Anthropic's frontend-design Skill Just Works (Medium, Ahmed Soliman)](https://medium.com/@ahmed.soliman/why-anthropics-frontend-design-skill-just-works-a-prompt-engineering-breakdown-72a1386df114)
- [Tells of a Slop UI (Hacker News)](https://news.ycombinator.com/item?id=49867038)
- [Slightly reducing the sloppiness of AI generated front end (Hacker News)](https://news.ycombinator.com/item?id=48504912)
- [Why AI Sucks at Front End (Hacker News)](https://news.ycombinator.com/item?id=47738864)
- [AI Slop Website Design: How to Stop Looking AI-Generated (Build to Launch, 120 pages audited)](https://buildtolaunch.substack.com/p/fix-ai-slop-website-design)
- [Vibe-Coded Website Examples (2026): Real Sites and the Tells (ohhwells)](https://www.ohhwells.com/blog/vibe-coded-website)
- [Vibe-Coded UI Patterns (vibemole examples)](https://vibemole.com/examples/vibe-coded-ui-patterns)
- [Why Vibe Coded Websites All Look the Same (codemyspec)](https://codemyspec.com/blog/vibe-coded-websites-look-the-same)
- [Why Do AI-Generated Websites All Look the Same? (Joshua Snoddy)](https://www.joshuasnoddy.com/blog/why-ai-websites-look-the-same/)
- [Vibe Coded Website vs Designed: How to Tell (Studio Maydit)](https://studiomaydit.com/blog/vibe-coded-website-vs-designed)
- [How to Make Your AI-Generated Website Look Professionally Designed (Differ)](https://blog.getdiffer.com/design-tips-vibe-coded-project)
- [Fix a Generic AI Landing Page: 6 Tells and the Prompts That Kill Them (SuperDesign)](https://superdesign.dev/blog/fix-generic-ai-landing-page)
- [Why SaaS Websites Look The Same (Overpass Studio)](https://www.overpass.studio/blog/why-saas-websites-look-the-same)
- [Linear Design System, Decoded: Tailwind + shadcn (buildmvpfast)](https://www.buildmvpfast.com/blog/linear-aesthetic-tokens-density-keyboard-first-ux-2026)
- [Linear-style website design: what makes it work (illos.dev)](https://illos.dev/guides/linear-style-design)
- [Everything built with shadcn/ui looks the same (laithjunaidy)](https://uxskill.laithjunaidy.com/blog/shadcn-ui-looks-generic.html)
- [The shadcn trap: why shadcn looks generic (designdotmd)](https://freedesignmd.com/blog/shadcn-looks-generic)
- [AI copy tells reference (marketingskills, ai-tells.md)](https://github.com/coreyhaines31/marketingskills/blob/main/skills/copywriting/references/ai-tells.md)
- [AI Writing Tells & What They Cost You (Emily Aborn)](https://emilyaborn.com/ai-writing-tells-what-they-cost-you/)
- [It's not [X], it's [Y] (Ruben Hassid)](https://ruben.substack.com/p/its-not-x-its-y)
- [How to Spot AI Writing Tells: 17 Examples (Olivia Cal)](https://www.oliviacal.com/post/ai-writing-tells)
- [Why AI Websites All Look the Same and How to Build Something Different (DEV Community)](https://dev.to/gdg/why-ai-websites-all-look-the-same-and-how-to-build-something-different-1gan)
