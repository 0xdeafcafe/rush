# The match composer

Status: proposed 2026-10-02. Built so far: the shelf on the Prompt's top
border (`internal/ui/shelf.go`), see "Built" at the end. Follows `providers-redesign.md`,
whose model (provider × harness × model, `agent.Compat`) it keeps; it
replaces how you *pick*, not what's picked.

## The idea

Halo MCC's "Compose your match": one PLAY button over a preset, rows of
tiles you tick, and a stats panel that recomputes from the ticks. Most days
you pick a profile ("Claude") and go. Sometimes you mix it up. Nothing is
random: you choose what you're willing to run, and rush shows what that
gives you.

## Why today feels messy

The unit is the pairing, `provider/harness` (a route), but no screen treats
it as one. Each page is about one kind of thing and lists its relationships
to the others:

- Settings › Providers lists each provider, then its harnesses
  (`settings_providers.go`).
- Settings › Harnesses lists each harness, then the providers it runs on
  (`settings_harnesses.go`).
- Settings › Profiles lists providers again, with a `RunsIn` per provider
  (`settings_profiles.go`).
- The start sheet has Profile, then Provider › Harness › Model columns
  (`startsheet.go`).
- Above the input box, the same "next agent" is shown three times: the
  footer block with recent setups (`footer.go:28`), the box border's chip
  (`view.go:2394`, `startWith`), and the cmdbar's "new agents:" line
  (`cmdbar.go:1335`).

So every pairing is shown at least twice and edited from both sides. And
`alt+w` (`profiles.go:189`) writes the global default, while the start
sheet's changes are one-offs: two meanings for what looks like one action.

## The design: a shelf over a composer

Two layers of one surface.

- **The shelf** is a profile: an ordered row of *complete* tiles. A tile is
  one runnable setup, `provider/harness · account · model · effort`. Picking
  a tile is one keypress. Ticking several never makes combinations you
  didn't ask for.
- **The composer** is how tiles get made. You tick provider tiles and
  harness tiles. The routes panel is every compatible pair of the ticks,
  as a tree under each provider. Enter on a route puts it on the shelf.
  It's the compatibility matrix you already like, read rather than
  navigated: each provider and each harness appears once.

### Above the input box (replaces the footer block, the chip, the cmdbar line)

```
  Work: [1 Claude ★] [2 Codex] [3 Local] [+]    ✻ cc · opus[1m] · high       alt+m ▾
╭──────────────────────────────────────────────────────────────────────────────────╮
│ describe a task                                                                  │
╰─ new agent in ~/src/rush ────────────────────────────────────────────────────────╯
```

- Enter starts the ★ tile. "Just Claude" costs nothing extra.
- `alt+1…9` makes that tile next, for this session only. Focus stays in the
  box. It never submits the draft.
- `alt+m` unfolds the strip upward, keeping the draft visible:

```
  Work: [1 Claude ★] [2 Codex] [3 Local] [+]
  ✻ Claude Code › Anthropic sub › alex › opus[1m] › high · ask          ● ready
  [Harness…] [Provider/account…] [Model…] [Effort…] [Permissions…]     alt+m full
```

- `alt+m` again opens the full composer. `esc` folds back.
- Narrow panes show the next tile and "+2".

The recent setups the footer shows today become ghost tiles at the end of
the shelf ("recent: pi·qwen"); `s` on one keeps it.

### The full composer (replaces the start sheet)

```
╭─ Compose ── rush / main ──────────────────────────── Profile ‹ Work (edited) › ─╮
│ ▌START  ✻ Claude Code · Anthropic sub · opus[1m] · high                 enter   │
│                                                                                 │
│ SHELF  [x 1 Claude ★] [x 2 Codex] [x 3 Local]       At limit ‹ next tile ›      │
│ ─────────────────────────────────────────────────────────────────────────────── │
│ PROVIDERS                                  │ ROUTES                             │
│ [✻ sub ●] [✻ API ○] [◎ sub ●] [◉ Ollama ●] │ ✻ Anthropic sub   ▓▓▓░ 41%         │
│ [◎ API ✗ add key]                          │  ├ ★ Claude Code  opus[1m] shelf   │
│ HARNESSES                                  │  └ ⚠ Pi           own login        │
│ [✻ Claude ●] [◎ Codex ●] [π Pi ●] [■ Vibe ░]│ ◎ OpenAI sub      ▓░░░ 22%         │
│                                            │  └ ★ Codex        gpt-6-astra shelf│
│ ░ Vibe adds nothing with these providers   │ ◉ Ollama                           │
│                                            │  └   Pi           qwen3-coder shelf│
│                                            │ EVERY ROUTE resume · images · MCP  │
│                                            │ ONLY SOME   subagents · rewind     │
│ same as  #new cc@anthropic-sub:alex opus[1m] high                               │
│ space tick · enter on a route → shelf · s save profile · i all pairs · esc      │
╰─────────────────────────────────────────────────────────────────────────────────╯
```

- **Routes** = for each ticked provider, in tile order, each ticked harness
  where `agent.Compat` allows it; the provider's ★ harness first.
- **Stats**: EVERY ROUTE is the intersection of the shelf's features, ONLY
  SOME the rest ("what do I lose if it falls over to Pi?"). Headroom is
  shown per account, never summed across unlike limits.
- **Invalid**: a tile is never invalid alone. Dim ░ means it adds no route
  with the current ticks, and the panel says why. ✗ means not usable here
  (no key, not installed), and space on it offers the fix. Zero routes turns
  START red with the nearest fix; enter does nothing until then.
- **Edits are one-offs.** Touching anything makes "Work (edited)". Enter
  starts it once. `s` saves over the profile or as a new one; the save
  dialog has "use in ‹this folder›", which absorbs folder rules.
- `i` shows the full provider × harness grid, read-only.
- The bottom line is the `#new` equivalent, so the composer teaches the
  typed form.

### Several tiles: a menu, not fall-over

Ticking tiles builds a menu. It never implies fall-over or a random pick.
Fall-over is the one explicit row "At limit ‹stay | next tile›", which is
today's `Profile.Mix`, and walks the shelf in order. `OnLimit` stays as it
is, in the save dialog.

## Data

`state.Profile` already holds most of it: `Providers` is ordered, `RunsIn`
gives each a harness. What it can't say is two tiles on one provider, or a
model and account for any tile but the first. So:

```go
// Tile is one runnable setup on a profile's shelf.
type Tile struct {
	Provider string `json:"provider"`
	Harness  string `json:"harness,omitempty"` // "" is Config.RunsIn's
	Account  string `json:"account,omitempty"`
	Model    string `json:"model,omitempty"`
	Effort   string `json:"effort,omitempty"`
}
```

`Profile` gains `Tiles []Tile`. On load, a profile without tiles gets one
per provider from `Providers`, `RunsIn`, and (for the first) `Account`,
`Model`, `Effort`; `Billing` is already the split provider id. Those fields
stay readable and stop being written. Builtin profiles are one tile each.

## What goes

- The start sheet's Profile row and three columns (`startsheet.go` body).
  `openSetupSheet` stays for "switch to" inside a session.
- Settings › Profiles, and its `RunsIn` per provider.
- "Harnesses" on Settings › Providers and "Runs on" on Settings ›
  Harnesses. Those pages become inventories: accounts, keys, limits, models;
  install, sign-in, harness settings.
- The footer's next-agent block and recents, the border chip, the cmdbar's
  "new agents:" line.
- `alt+w` as a global-default writer. It opens the composer on the profile
  row; the default is set by ★ in the save dialog.

## Keys

- `alt+m` already is `list.start` and `session.start` (`keymap/defaults.go`).
  It keeps the meaning, and gains the unfold-then-full step.
- `alt+1…9` open a room member's session (`roomfeed.go:126`). That's in a
  room, not the list's input box, but check they don't collide before
  binding them in `List`.
- `alt+w` is `profile.pick`; its title changes with its behaviour.

## Build order

1. `Tile`, the migration, and `routes(providers, harnesses)` over
   `agent.Compat`, with tests. No UI.
2. The strip above the input box. It deletes the footer block, chip and
   cmdbar line in the same change.
3. The full composer, replacing the start sheet body.
4. The Settings cuts, and `alt+w`.

## Built (2026-10-02)

The shelf, on the Prompt's top border where the setup chip was, rather
than a row of its own above the box, so the list keeps its height:

```
╭─ new agent in rush ──── ✻ Anthropic (Claude Code) · personal · Opus  ⌥2 OpenAI ⌥3 Moonshot ⌥4 Ollama·codex ─╮
```

- The chip is what the next session starts as; the others are numbered.
  `⌥1…9` or a click takes one for the next session only; `⌥1` (the
  profile's own) clears the one-off. Numbers don't move when you pick.
- Tiles are the profile's installed providers in order, then the fleet's
  recent setups (`recentSetups`), each once, up to nine. A one-off from
  the start sheet or `#new` that isn't on the shelf shows first, unnumbered.
- Too narrow: tiles drop from the end; the chip stays.
- `Tile` isn't stored yet: nothing saves a shelf until the composer does.

The full composer (`internal/ui/matchsheet.go`) is ⌥m, shift+tab, `/agent`
and the footer's next agent: START, provider and harness tiles you tick, the
routes they make as a tree with each route's model on ←→, and what every
route does against what only some do. Ticking a tile on moves START's
choice to what the tick turns on: the choice's own provider or harness when
they meet, else the ticked provider's default route. `/model`, `/effort`
and switching a running session keep the start sheet.

Not yet: account, effort and permissions in the composer, saving ticks as a
profile, the alt+m unfold, the Settings cuts, and
retiring the side list's footer and the cmdbar's "new agents:" line.
