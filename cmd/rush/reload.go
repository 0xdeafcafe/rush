package main

import (
	"fmt"

	"github.com/0xdeafcafe/rush/internal/host"
)

// reloadAgents is rush reload's second half: every session's host on an
// older rush that's between turns, restarted on this one, as Settings →
// Updates does when you press Enter. A session mid turn finishes its turn
// first: the leading view restarts it then. One the installed rush can't
// run stays on the one it has, and is counted failed: exit status 1.
func reloadAgents() {
	var l host.Lister
	stale, restarted, failed := host.ReloadAgents(&l, host.ExeStamp())
	fmt.Printf("restarted %d agent(s) on this rush", restarted)
	if failed > 0 {
		fmt.Printf("; %d failed and carry on as they were: their events say why", failed)
		stale -= failed
	}
	if stale > 0 {
		fmt.Printf("; %d mid-turn or waiting on you, restarted once their turn ends", stale)
	}
	fmt.Println()
	if failed > 0 {
		exit(1)
	}
}
