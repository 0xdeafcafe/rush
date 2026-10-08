package main

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
)

// What's in Sam's repositories, and what their agents said and did.

var checkoutFiles = map[string]string{
	"package.json": `{
  "name": "@acme/checkout",
  "private": true,
  "scripts": { "dev": "vite", "test": "vitest run", "build": "vite build" }
}
`,
	"src/payments/index.ts": `export * from "./card";
export * from "./providers";
`,
	"src/payments/card.ts": `import type { Provider, Payment } from "./types";

export const card: Provider = {
  id: "card",
  label: "Card",
  async pay(p: Payment) {
    const intent = await p.api.createIntent({ amount: p.total, currency: p.currency });
    return p.api.confirm(intent.id, p.method);
  },
};
`,
	"src/payments/providers.ts": `import { card } from "./card";

export const providers = [card];
`,
	"src/checkout/CheckoutPage.tsx": checkoutPageOld,
	"config/payments.json":          paymentsJSONOld,
	"README.md":                     "# checkout\n\nAcme's checkout: basket, delivery, payment.\n",
}

const checkoutPageOld = `import { useFlags } from "@acme/flags";
import { providers } from "../payments";
import { PayButton } from "./PayButton";

export function CheckoutPage({ basket }: Props) {
  const flags = useFlags();
  return (
    <section className="checkout">
      <Summary basket={basket} />
      {providers.map((p) => (
        <PayButton key={p.id} provider={p} total={basket.total} />
      ))}
    </section>
  );
}
`

const checkoutPageNew = `import { useFlags } from "@acme/flags";
import { providers } from "../payments";
import { applePay, canUseApplePay } from "../payments/apple-pay";
import { PayButton } from "./PayButton";

export function CheckoutPage({ basket }: Props) {
  const flags = useFlags();
  const wallets = flags.payments.applePay && canUseApplePay() ? [applePay] : [];
  return (
    <section className="checkout">
      <Summary basket={basket} />
      {[...wallets, ...providers].map((p) => (
        <PayButton key={p.id} provider={p} total={basket.total} />
      ))}
    </section>
  );
}
`

const paymentsJSONOld = `{
  "providers": {
    "gb": { "methods": ["card"] },
    "de": { "methods": ["card", "sepa"] },
    "us": { "methods": ["card"] }
  }
}
`

const paymentsJSONNew = `{
  "providers": {
    "gb": { "methods": ["card", "apple_pay"], "merchant": { "id": "merchant.example.acme.gb" } },
    "de": { "methods": ["card", "sepa", "apple_pay"], "merchant": { "id": "merchant.example.acme.de" } },
    "us": { "methods": ["card"] }
  }
}
`

const providersTS = `import { card } from "./card";

// Providers each region offers, in the order the page shows them.
export const providers = [card];
`

const applePayTS = `import type { Provider, Payment } from "./types";
import config from "../../config/payments.json";

// Apple Pay, where the browser offers it and the region has a merchant.
export function canUseApplePay(): boolean {
  return typeof window !== "undefined" && "ApplePaySession" in window && ApplePaySession.canMakePayments();
}

export const applePay: Provider = {
  id: "apple_pay",
  label: "Apple Pay",
  async pay(p: Payment) {
    const merchant = config.providers[p.region]?.merchant;
    if (!merchant) throw new Error(` + "`no Apple Pay merchant for ${p.region}`" + `);
    const session = new ApplePaySession(14, {
      countryCode: p.region.toUpperCase(),
      currencyCode: p.currency,
      merchantCapabilities: ["supports3DS"],
      supportedNetworks: ["visa", "masterCard", "amex"],
      total: { label: "Acme", amount: (p.total / 100).toFixed(2) },
    });
    session.onvalidatemerchant = async (e) => {
      const res = await p.api.validateMerchant({ url: e.validationURL, merchantId: merchant.id });
      session.completeMerchantValidation(res);
    };
    return new Promise((resolve, reject) => {
      session.onpaymentauthorized = async (e) => {
        const ok = await p.api.confirmToken(e.payment.token);
        session.completePayment(ok ? ApplePaySession.STATUS_SUCCESS : ApplePaySession.STATUS_FAILURE);
        ok ? resolve(ok) : reject(new Error("declined"));
      };
      session.begin();
    });
  },
};
`

var apiFiles = map[string]string{
	"go.mod":           "module example.com/lumen/api\n\ngo 1.27\n",
	"cmd/api/main.go":  "package main\n\nfunc main() { serve() }\n",
	"internal/http.go": "package internal\n",
	"README.md":        "# lumen-api\n",
}

const limitGo = `package limit

import (
	"sync"
	"time"
)

// Bucket lets n requests through per window, per key.
type Bucket struct {
	mu     sync.Mutex
	n      int
	window time.Duration
	seen   map[string][]time.Time
}
`

var dsFiles = map[string]string{"package.json": "{\"name\": \"@acme/design-system\"}\n", "tokens/colours.css": ":root {}\n", "README.md": "# design-system\n"}
var orbitFiles = map[string]string{"Cargo.toml": "[package]\nname = \"orbit\"\nversion = \"0.9.0\"\n", "src/main.rs": "fn main() {}\n", "README.md": "# orbit\n"}
var notesFiles = map[string]string{"README.md": "# field notes\n", "retries.md": "# on retries\n"}

// codexReview is the session Codex's review runs in.
const codexReview = "0199a8f2-5c3e-7d1a-9b2c-4e5f6a7b8c9d"

// codexPrompt is what the featured session asked Codex to review.
const codexPrompt = "review src/payments/apple-pay.ts for anything that would fail a PCI review, and say what you'd change"

// patch is Claude Code's structured account of an edit: one hunk.
func patch(file string, oldStart int, lines ...string) map[string]any {
	var o, n int
	for _, l := range lines {
		switch l[0] {
		case '-':
			o++
		case '+':
			n++
		default:
			o++
			n++
		}
	}
	return map[string]any{"filePath": file, "structuredPatch": []any{map[string]any{
		"oldStart": oldStart, "oldLines": o, "newStart": oldStart, "newLines": n, "lines": lines}}}
}

// read is a Read's result, numbered as Claude Code numbers it.
func read(body string) string {
	var b strings.Builder
	for i, l := range strings.Split(strings.TrimRight(body, "\n"), "\n") {
		fmt.Fprintf(&b, "%6d→%s\n", i+1, l)
	}
	return b.String()
}

// featured is the session the README shows most: Apple Pay for Acme's
// checkout, with subagents, Codex asked for a review, and a step that
// failed.
func featured(dir string, ago func(time.Duration) time.Time) *conv {
	f := func(p string) string { return filepath.Join(dir, p) }
	subs := []sub{
		{id: "a1f0c2e4b6d8", toolUse: "toolu_sub_map", kind: "Explore", desc: "Map the payment providers", start: ago(47 * time.Minute), done: true,
			steps: []step{
				{tool: "Grep", input: map[string]any{"pattern": "Provider", "path": f("src")}, result: "src/payments/card.ts\nsrc/payments/providers.ts\nsrc/payments/types.ts"},
				{tool: "Read", input: map[string]any{"file_path": f("src/payments/types.ts")}, result: read("export interface Provider {\n  id: string;\n  pay(p: Payment): Promise<Receipt>;\n}")},
			},
			answer: "One Provider interface in src/payments/types.ts; card is the only provider, registered in providers.ts. Regions come from config/payments.json."},
		{id: "b2e1d3f5a7c9", toolUse: "toolu_sub_merchant", kind: "general-purpose", desc: "Write Apple Pay merchant validation", start: ago(46 * time.Minute), done: true,
			steps: []step{
				{tool: "Read", input: map[string]any{"file_path": f("src/api/payments.ts")}, result: read("export async function validateMerchant() {}")},
				{tool: "Edit", input: map[string]any{"file_path": f("src/api/payments.ts"), "old_string": "{}", "new_string": "{ … }"}, result: "The file has been updated."},
				{tool: "Bash", input: map[string]any{"command": "pnpm vitest run src/api", "description": "Run the API tests"}, result: " ✓ src/api/payments.test.ts (9 tests) 212ms\n\n Test Files  1 passed (1)\n      Tests  9 passed (9)"},
			},
			answer: "validateMerchant posts the validation URL and merchant id to /payments/apple-pay/session and returns Apple's opaque session. 9 tests pass."},
		{id: "c3d2e4a6b8f0", toolUse: "toolu_sub_e2e", kind: "general-purpose", desc: "Check the checkout e2e tests", start: ago(46 * time.Minute), done: true,
			steps: []step{
				{tool: "Glob", input: map[string]any{"pattern": "e2e/**/*.spec.ts"}, result: "e2e/checkout.spec.ts\ne2e/basket.spec.ts"},
				{tool: "Read", input: map[string]any{"file_path": f("e2e/checkout.spec.ts")}, result: read("test('pays by card', async ({ page }) => {})")},
			},
			answer: "e2e/checkout.spec.ts counts the pay buttons: it expects exactly one. It needs to allow for Apple Pay when the flag is on."},
		{id: "d4c3b5a7e9f1", toolUse: "toolu_sub_notes", kind: "general-purpose", desc: "Draft the rollout notes", start: ago(90 * time.Second),
			steps: []step{
				{tool: "Read", input: map[string]any{"file_path": f("docs/rollouts/TEMPLATE.md")}, result: read("# Rollout\n\n## What changes\n\n## How we'll know it works")},
				{tool: "Write", input: map[string]any{"file_path": f("docs/rollouts/apple-pay.md"), "content": "# Apple Pay\n"}, result: "File created successfully", running: true},
			}},
	}
	script := `python3 - <<'EOF'
import json
cfg = json.load(open('config/payments.json'))
for region, p in cfg['providers'].items():
    if 'apple_pay' in p['methods']:
        print(region, p['merchant']['id'])
EOF
grep -rn "merchantId" src/payments | head`
	return &conv{id: "5e1b7a3c-2d4f-4a8b-9c6d-1e2f3a4b5c6d", cwd: dir, branch: "feat/apple-pay", model: "claude-opus-5-5", subs: subs,
		turns: []turn{
			{prompt: "how is checkout wired for payments? I want Apple Pay next to cards, behind the payments.applePay flag", at: ago(58 * time.Minute),
				steps: []step{
					{text: "I'll look at how card payments are wired first.", tool: "Grep", input: map[string]any{"pattern": "payByCard|Provider", "path": f("src")},
						result: "src/payments/card.ts:3\nsrc/payments/providers.ts:1\nsrc/checkout/CheckoutPage.tsx:2"},
					{tool: "Read", input: map[string]any{"file_path": f("src/payments/card.ts")}, result: read(checkoutFiles["src/payments/card.ts"])},
					{tool: "Read", input: map[string]any{"file_path": f("src/checkout/CheckoutPage.tsx")}, result: read(checkoutPageOld)},
					{tool: "Read", input: map[string]any{"file_path": f("config/payments.json")}, result: read(paymentsJSONOld)},
				},
				answer: "Every payment method is a **Provider** (`src/payments/types.ts`): an id, a label and `pay()`. The checkout page draws a button for each one in `providers`, so Apple Pay fits in as a provider of its own, shown first when the flag is on and the browser can use it.\n\n" +
					"| File | What changes | Risk |\n|---|---|---|\n" +
					"| `src/payments/apple-pay.ts` | new: the session and merchant validation | medium |\n" +
					"| `src/checkout/CheckoutPage.tsx` | the button, behind `payments.applePay` | low |\n" +
					"| `config/payments.json` | a merchant id per region | high: a wrong id fails at the till |\n" +
					"| `e2e/checkout.spec.ts` | expects one pay button today | low |\n\n" +
					"**Next:** merchant validation needs a server route, so that's the one piece outside this repo's front end."},
			{prompt: "sounds good. split it up with subagents, and have codex review it once it's in", at: ago(48 * time.Minute),
				steps: []step{
					{tool: "TodoWrite", input: map[string]any{"todos": []any{
						map[string]any{"content": "Map the payment providers", "status": "completed", "activeForm": "Mapping the providers"},
						map[string]any{"content": "Merchant validation route", "status": "completed", "activeForm": "Writing merchant validation"},
						map[string]any{"content": "Apple Pay provider and button", "status": "in_progress", "activeForm": "Adding the provider"},
						map[string]any{"content": "Codex review", "status": "pending", "activeForm": "Asking Codex for a review"},
						map[string]any{"content": "Rollout notes", "status": "pending", "activeForm": "Drafting the rollout notes"},
					}}, result: "Todos have been modified successfully."},
					{tool: "Agent", input: map[string]any{"description": subs[0].desc, "subagent_type": subs[0].kind, "prompt": "Find every payment provider and how the checkout picks them."}, result: subs[0].answer, took: 58 * time.Second, out: 1400},
					{tool: "Agent", input: map[string]any{"description": subs[1].desc, "subagent_type": subs[1].kind, "prompt": "Add a merchant validation endpoint for Apple Pay, with tests."}, result: subs[1].answer, took: 3 * time.Minute, out: 2600},
					{tool: "Agent", input: map[string]any{"description": subs[2].desc, "subagent_type": subs[2].kind, "prompt": "Say what the e2e tests assume about pay buttons."}, result: subs[2].answer, took: 71 * time.Second, out: 1100},
				},
				answer: "All three are back:\n\n- **Providers**: card is the only one; Apple Pay registers the same way.\n- **Merchant validation**: `/payments/apple-pay/session` is in, with 9 passing tests.\n- **e2e**: `checkout.spec.ts` expects exactly one pay button, so it needs to know about the flag.\n\nI'll wire the page up next."},
			{prompt: "wire it into the checkout page and run the payments tests", at: ago(11 * time.Minute), open: true,
				steps: []step{
					{text: "Adding the provider, then the button behind the flag.", tool: "Write", input: map[string]any{"file_path": f("src/payments/apple-pay.ts"), "content": applePayTS},
						result: "File created successfully at: " + f("src/payments/apple-pay.ts"), extra: map[string]any{"type": "create", "filePath": f("src/payments/apple-pay.ts"), "content": applePayTS, "structuredPatch": []any{}}},
					{tool: "Edit", input: map[string]any{"file_path": f("src/checkout/CheckoutPage.tsx"), "old_string": "import { providers } from \"../payments\";", "new_string": "…"},
						result: "The file has been updated.", extra: patch(f("src/checkout/CheckoutPage.tsx"), 1,
							` import { useFlags } from "@acme/flags";`, ` import { providers } from "../payments";`,
							`+import { applePay, canUseApplePay } from "../payments/apple-pay";`, ` import { PayButton } from "./PayButton";`, ` `,
							` export function CheckoutPage({ basket }: Props) {`, `   const flags = useFlags();`,
							`+  const wallets = flags.payments.applePay && canUseApplePay() ? [applePay] : [];`, `   return (`)},
					{tool: "Bash", input: map[string]any{"command": "pnpm vitest run src/payments", "description": "Run the payments tests"},
						result: " ✓ src/payments/card.test.ts (11 tests) 184ms\n ✓ src/payments/apple-pay.test.ts (6 tests) 97ms\n\n Test Files  2 passed (2)\n      Tests  17 passed (17)", took: 6 * time.Second},
					{text: "Tests pass. Checking every region that offers Apple Pay has a merchant id.", tool: "Bash", input: map[string]any{"command": script, "description": "List the merchant id of each Apple Pay region"},
						result: "Exit code 1\nde merchant.example.acme.de\ngb merchant.example.acme.gb\nTraceback (most recent call last):\n  File \"<stdin>\", line 5, in <module>\nKeyError: 'merchant'", isErr: true, took: 1700 * time.Millisecond},
					{tool: "Read", input: map[string]any{"file_path": f("config/payments.json")}, result: read(paymentsJSONNew)},
					{text: "The US has no merchant yet, which the script tripped on; the page already hides Apple Pay there. Asking Codex for its review.", tool: "Bash",
						input: map[string]any{"command": `codex exec "` + codexPrompt + `"`, "description": "Ask Codex to review apple-pay.ts", "run_in_background": false}, running: true},
					{tool: "Agent", input: map[string]any{"description": subs[3].desc, "subagent_type": subs[3].kind, "prompt": "Draft docs/rollouts/apple-pay.md from the template.", "run_in_background": true}, running: true},
				}},
		}}
}

// rateLimits is the session that shows how rush draws a session's work:
// code it read and wrote, highlighted; a failing test run and its fix; a
// commit, a merge and a push as git's own cards; and a chain of checks
// running now, each command timed.
func rateLimits(dir string, ago func(time.Duration) time.Time) *conv {
	f := func(p string) string { return filepath.Join(dir, p) }
	return &conv{id: "a41c09e2-7b3d-4e5f-8a9b-0c1d2e3f4a5b", cwd: dir, branch: "feat/rate-limits", model: "claude-sonnet-5",
		turns: []turn{
			{prompt: "rate limit the public API: 100 requests a minute per key, 429 with Retry-After past it", at: ago(25 * time.Minute),
				steps: []step{
					{tool: "Grep", input: map[string]any{"pattern": "HandleFunc", "path": f("internal")}, result: "internal/http.go:14\ninternal/http.go:15\ninternal/http.go:16"},
					{tool: "Read", input: map[string]any{"file_path": f("internal/http.go")}, result: read(httpGo)},
					{tool: "WebSearch", input: map[string]any{"query": "Retry-After header seconds or HTTP-date RFC 9110"},
						result: "RFC 9110 §10.2.3: Retry-After is either an HTTP-date or a number of seconds to wait. Seconds are simpler for clients to act on."},
					{text: "A token bucket per key, in its own package, with the middleware wrapping every public route.", tool: "Write",
						input:  map[string]any{"file_path": f("internal/limit/limit.go"), "content": limitGo},
						result: "File created successfully at: " + f("internal/limit/limit.go"), extra: map[string]any{"type": "create", "filePath": f("internal/limit/limit.go"), "content": limitGo, "structuredPatch": []any{}}},
					{tool: "Edit", input: map[string]any{"file_path": f("internal/http.go"), "old_string": "mux.HandleFunc", "new_string": "…"},
						result: "The file has been updated.", extra: patch(f("internal/http.go"), 11,
							` func routes(db *store.DB) http.Handler {`, ` 	mux := http.NewServeMux()`,
							`-	mux.HandleFunc("GET /v2/ledger", ledger(db))`, `-	mux.HandleFunc("POST /v2/import", importCSV(db))`,
							`+	lim := limit.New(100, time.Minute)`, `+	mux.Handle("GET /v2/ledger", lim.Wrap(ledger(db)))`, `+	mux.Handle("POST /v2/import", lim.Wrap(importCSV(db)))`,
							` 	return mux`, ` }`)},
					{tool: "Bash", input: map[string]any{"command": "go test ./internal/limit/", "description": "Test the limiter"}, isErr: true, took: 3 * time.Second,
						result: "--- FAIL: TestRetryAfter (0.00s)\n    limit_test.go:41: Retry-After = \"0\", want \"60\"\nFAIL\nFAIL\tgithub.com/lumen-labs/lumen-api/internal/limit\t0.18s\n"},
					{text: "Retry-After should be the time until the bucket refills, not what's left of it.", tool: "Edit",
						input:  map[string]any{"file_path": f("internal/limit/limit.go"), "old_string": "left", "new_string": "…"},
						result: "The file has been updated.", extra: patch(f("internal/limit/limit.go"), 38,
							` 	if b.tokens < 1 {`, `-		w.Header().Set("Retry-After", strconv.Itoa(int(b.tokens)))`,
							`+		wait := time.Until(b.refill).Round(time.Second)`, `+		w.Header().Set("Retry-After", strconv.Itoa(int(wait.Seconds())))`,
							` 		http.Error(w, "too many requests", http.StatusTooManyRequests)`)},
					{tool: "Bash", input: map[string]any{"command": "go test ./...", "description": "Run the tests"}, took: 9 * time.Second,
						result: "ok  \tgithub.com/lumen-labs/lumen-api/internal\t0.41s\nok  \tgithub.com/lumen-labs/lumen-api/internal/limit\t0.22s\nok  \tgithub.com/lumen-labs/lumen-api/internal/ledger\t1.37s\n"},
					{tool: "Bash", input: map[string]any{"command": `git add -A && git commit -m "feat(api): rate limit the public API per key"`, "description": "Commit the rate limits"}, took: time.Second,
						result: "[feat/rate-limits 3f9c2e1] feat(api): rate limit the public API per key\n 3 files changed, 142 insertions(+), 6 deletions(-)\n create mode 100644 internal/limit/limit.go\n create mode 100644 internal/limit/limit_test.go\n"},
				},
				answer: "Every public route now goes through `limit.Wrap`: **100 requests a minute per API key**, a 429 past it, with `Retry-After` in seconds until the bucket refills. The test that caught the wrong `Retry-After` passes, and it's committed as `3f9c2e1`."},
			{prompt: "merge main in, push it, and run the full checks", at: ago(80 * time.Second), open: true,
				steps: []step{
					{tool: "Bash", input: map[string]any{"command": "git fetch -q origin && git merge origin/main", "description": "Merge main in"}, took: 2 * time.Second,
						result: "Merge made by the 'ort' strategy.\n internal/auth/refresh.go      | 18 ++++++++++++------\n internal/auth/refresh_test.go | 24 ++++++++++++++++++++++++\n 2 files changed, 36 insertions(+), 6 deletions(-)\n"},
					{tool: "Bash", input: map[string]any{"command": "git log --oneline -4", "description": "Show the history"},
						result: "c81d0a4 (HEAD -> feat/rate-limits) Merge remote-tracking branch 'origin/main' into feat/rate-limits\n9b2d4e7 (origin/main, main) fix(auth): refresh tokens rotate\n3f9c2e1 feat(api): rate limit the public API per key\n5a7e1c3 perf(import): stream the CSV\n"},
					{tool: "Bash", input: map[string]any{"command": "git push -u origin feat/rate-limits", "description": "Push the branch"}, took: 2 * time.Second,
						result: "To github.com:lumen-labs/lumen-api.git\n * [new branch]      feat/rate-limits -> feat/rate-limits\nbranch 'feat/rate-limits' set up to track 'origin/feat/rate-limits'.\n"},
					{tool: "Bash", input: map[string]any{"command": checksCmd, "description": "Vet, race-test and lint"}, running: true},
				}},
		}}
}

// migrationNotes is a session on a local model, through Pi.
func migrationNotes(dir string, ago func(time.Duration) time.Time) *conv {
	f := func(p string) string { return filepath.Join(dir, p) }
	return &conv{id: "b3c4d5e6-1a2b-4c3d-9e8f-7a6b5c4d3e2f", cwd: dir, branch: "main", model: "qwen3-coder:30b",
		turns: []turn{{prompt: "write migration notes for the ledger schema change, from the last three migrations", at: ago(6 * time.Minute), open: true,
			steps: []step{
				{tool: "Bash", input: map[string]any{"command": "ls migrations | tail -3", "description": "List the newest migrations"},
					result: "0041_ledger_currency.sql\n0042_ledger_entry_index.sql\n0043_ledger_split_amount.sql\n"},
				{tool: "Read", input: map[string]any{"file_path": f("migrations/0043_ledger_split_amount.sql")}, result: read(splitSQL)},
				{text: "0043 splits amount into minor units and currency; reads need both columns from here on.", tool: "Write",
					input:   map[string]any{"file_path": f("docs/migrations/ledger.md"), "content": "# Ledger migrations\n"},
					running: true},
			}}}}
}

const splitSQL = `-- Amounts become minor units and a currency, so sums never round.
ALTER TABLE ledger_entries ADD COLUMN amount_minor BIGINT;
ALTER TABLE ledger_entries ADD COLUMN currency CHAR(3) NOT NULL DEFAULT 'GBP';
UPDATE ledger_entries SET amount_minor = ROUND(amount * 100);
ALTER TABLE ledger_entries ALTER COLUMN amount_minor SET NOT NULL;
ALTER TABLE ledger_entries DROP COLUMN amount;
`

// checksCmd is the chain the rate limits session runs now.
const checksCmd = "go vet ./... && go test -race ./... && golangci-lint run"

const httpGo = `package internal

import (
	"net/http"

	"github.com/lumen-labs/lumen-api/internal/store"
)

// routes is every public route.
func routes(db *store.DB) http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("GET /v2/ledger", ledger(db))
	mux.HandleFunc("POST /v2/import", importCSV(db))
	return mux
}
`

func taxRounding(dir string, ago func(time.Duration) time.Time) *conv {
	return &conv{id: "b3a4c5d6-8e9f-4a0b-9c1d-2e3f4a5b6c7d", cwd: dir, branch: "fix/tax-rounding", model: "claude-opus-5-5",
		turns: []turn{{prompt: "the tax rounding test fails about one run in twenty. find out why", at: ago(34 * time.Minute), open: true,
			steps: []step{
				{tool: "Bash", input: map[string]any{"command": "for i in $(seq 50); do pnpm vitest run src/tax --silent || echo FAIL; done | grep -c FAIL", "description": "Run the tax tests 50 times"}, result: "3", took: 2 * time.Minute},
				{text: "It fails when the random basket has a line ending in exactly half a penny: the test rounds half to even, the code rounds half up.", tool: "AskUserQuestion",
					input: map[string]any{"questions": []any{map[string]any{"question": "Round half up, or half to even?", "header": "Rounding", "multiSelect": false,
						"options": []any{map[string]any{"label": "Half up", "description": "What the invoices do today"}, map[string]any{"label": "Half to even", "description": "What HMRC's examples use"}}}}}, running: true},
			}}}}
}

func react19(dir string, ago func(time.Duration) time.Time) *conv {
	return &conv{id: "d0e1f2a3-9b8c-4d7e-8f6a-5b4c3d2e1f0a", cwd: dir, branch: "main", model: "claude-opus-5-5",
		turns: []turn{{prompt: "upgrade the storefront to React 19 and fix what breaks", at: ago(3 * time.Hour),
			steps: []step{
				{tool: "Bash", input: map[string]any{"command": "pnpm up react@19 react-dom@19", "description": "Upgrade React"}, result: "+ react 19.2.1\n+ react-dom 19.2.1", took: 14 * time.Second},
				{tool: "Bash", input: map[string]any{"command": "pnpm tsc --noEmit", "description": "Typecheck"}, result: "Found 23 errors in 9 files.", isErr: true, took: 21 * time.Second},
			},
			answer: "Claude usage limit reached. Your limit will reset at " + limitReset(ago).Format("3pm") + "."}}}
}

// limitReset is when the session limit of the React upgrade's account resets.
func limitReset(ago func(time.Duration) time.Time) time.Time {
	return ago(-2*time.Hour - 20*time.Minute).Truncate(time.Hour)
}

// codex writes Codex's own sessions: the review the featured session
// asked for (running now), and one from yesterday.
func (w *world) codex(applePay, ds string, ago func(time.Duration) time.Time) error {
	type item = map[string]any
	rollout := func(id, cwd, source string, at time.Time, prompt string, rest []item, answer string) error {
		dir := filepath.Join(w.home, ".codex", "sessions", at.Format("2006"), at.Format("01"), at.Format("02"))
		if err := os.MkdirAll(dir, 0o755); err != nil {
			return err
		}
		lines := []item{
			{"type": "session_meta", "payload": item{"id": id, "timestamp": at.UTC().Format(time.RFC3339), "cwd": cwd, "cli_version": "0.98.0", "source": source}},
			{"type": "turn_context", "payload": item{"model": "gpt-5.5-codex", "approval_policy": "never"}},
			{"type": "response_item", "payload": item{"type": "message", "role": "user", "content": []any{item{"type": "input_text", "text": prompt}}}},
			{"type": "event_msg", "payload": item{"type": "task_started"}},
		}
		lines = append(lines, rest...)
		if answer != "" {
			lines = append(lines, item{"type": "response_item", "payload": item{"type": "message", "role": "assistant", "content": []any{item{"type": "output_text", "text": answer}}}},
				item{"type": "event_msg", "payload": item{"type": "task_complete"}})
		}
		var out []string
		t := at
		for _, l := range lines {
			t = t.Add(3 * time.Second)
			l["timestamp"] = t.UTC().Format("2006-01-02T15:04:05.000Z")
			b, _ := jsonx.Marshal(l)
			out = append(out, string(b))
		}
		p := filepath.Join(dir, "rollout-"+at.Format("2006-01-02T15-04-05")+"-"+id+".jsonl")
		if err := writeLines(p, out); err != nil {
			return err
		}
		return os.Chtimes(p, t, t)
	}
	call := func(id, cmd string) item {
		args, _ := jsonx.Marshal(item{"command": []string{"bash", "-lc", cmd}})
		return item{"type": "response_item", "payload": item{"type": "function_call", "name": "shell", "call_id": id, "arguments": string(args)}}
	}
	output := func(id, text string) item {
		return item{"type": "response_item", "payload": item{"type": "function_call_output", "call_id": id, "output": text}}
	}
	reason := func(text string) item {
		return item{"type": "response_item", "payload": item{"type": "reasoning", "summary": []any{item{"type": "summary_text", "text": text}}}}
	}
	if err := rollout(codexReview, applePay, "exec", ago(40*time.Second), codexPrompt, []item{
		reason("**Reading the provider and its config**"),
		call("call_1", "sed -n 1,80p src/payments/apple-pay.ts"),
		output("call_1", applePayTS),
		reason("**Checking where the payment token goes**"),
		call("call_2", "rg -n \"confirmToken|token\" src/api"),
		output("call_2", "src/api/payments.ts:41:  const { confirmToken } = await req.json()\nsrc/api/payments.ts:58:  await stripe.paymentIntents.confirm(id, { confirmation_token: confirmToken })"),
		reason("**Checking the token is never logged**"),
	}, ""); err != nil {
		return err
	}
	return rollout("0199a1b2-3c4d-7e5f-8a9b-0c1d2e3f4a5b", ds, "cli", ago(22*time.Hour), "audit the colour tokens for contrast against WCAG AA", []item{
		call("call_1", "node scripts/contrast.mjs tokens/colours.css"),
		output("call_1", "14 pairs checked, 3 below 4.5:1"),
	}, "Three pairs fall short of AA: grey-500 on grey-100 (3.9:1), and both link colours on the dark surface. I've proposed darker values in tokens/colours.css.")
}
