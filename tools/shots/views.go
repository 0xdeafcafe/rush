package main

import (
	"os"
	"path/filepath"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/host"
)

// agentViews gives the featured session two views of its own, as its
// agent would keep them with rush view set.
func (w *world) agentViews() {
	es, _ := os.ReadDir(host.Root())
	for _, e := range es {
		var info host.Info
		b, err := os.ReadFile(filepath.Join(host.Root(), e.Name(), "info.json"))
		if err != nil || jsonx.Unmarshal(b, &info) != nil || info.Name != "add Apple Pay to checkout" {
			continue
		}
		f := func(v float64) *float64 { return &v }
		_ = host.SaveView(info.ID, host.View{Name: "tests", Title: "Payments tests", Summary: "17 passed",
			Stats: []host.Stat{{Value: "17", Label: "passed", Tone: "good"}}})
		time.Sleep(10 * time.Millisecond) // the rollout is the newest, so it shows first
		_ = host.SaveView(info.ID, host.View{Name: "rollout", Title: "Apple Pay rollout", Summary: "3 of 5 regions · US blocked",
			Progress: &host.Progress{Done: 3, Total: 5, Label: "regions live", Tone: "good"},
			Stats: []host.Stat{{Value: "3", Label: "live", Tone: "good", Trend: []float64{0, 1, 1, 2, 3}},
				{Value: "1", Label: "blocked", Tone: "bad"}, {Value: "1", Label: "to go", Tone: "quiet"}},
			Groups: []host.Group{
				{Title: "Blocked", Tone: "bad", Text: "The US has no merchant id yet, so Apple Pay stays hidden there.", Rows: []host.Row{
					{Text: "us", Meta: "no merchant id", Tone: "bad", Link: "https://developer.apple.com/apple-pay/"}}},
				{Title: "Checkout conversion", Rows: []host.Row{
					{Text: "card", Value: f(61), Meta: "61%"},
					{Text: "Apple Pay", Value: f(74), Meta: "74%", Tone: "good"},
					{Text: "PayPal", Value: f(48), Meta: "48%"}}},
				{Title: "Live", Tone: "good", Rows: []host.Row{
					{Text: "gb", Meta: "since 09:12", Tone: "good"},
					{Text: "ie", Meta: "since 09:40", Tone: "good"},
					{Text: "fr", Meta: "since 10:05", Tone: "good"}}},
				{Title: "Next", Folded: true, Rows: []host.Row{{Text: "de", Tone: "quiet"}}}},
			Note: "Updated as each region goes live."})
		return
	}
}
