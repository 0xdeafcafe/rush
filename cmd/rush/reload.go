package main

import (
	"fmt"

	"github.com/0xdeafcafe/rush/internal/host"
)

// reloadAgents is rush reload --agents: every session's host on an older
// rush that's between turns, restarted on this one, as Settings → Updates
// does when you press Enter. A session mid turn is left to finish; run it
// again later for those.
func reloadAgents() {
	var l host.Lister
	stale, restarted := l.Sweep(host.ExeStamp(), true)
	fmt.Printf("restarted %d agent(s) on this rush", restarted)
	if stale > 0 {
		fmt.Printf("; %d mid-turn or waiting on you, left as they are: run it again once they're between turns", stale)
	}
	fmt.Println()
}
