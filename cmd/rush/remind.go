package main

import (
	"crypto/rand"
	"encoding/hex"
	"fmt"
	"io"
	"os"
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/remind"
)

const remindUsage = `rush remind — have rush send a session a message at a time, waking it if it sleeps

  rush remind <when> <text>   remind this session at when: "25m", "2h", "15:30",
                              "3pm", "tomorrow 9:00", "2026-11-02 09:00"
  rush remind list            this session's reminders (--all: every session's)
  rush remind cancel <id>     drop one; "all" drops every one this session has

Run it in a session rush runs, or name one with --session <id>.
`

// remindCmd is rush remind.
func remindCmd(args []string, stdout, stderr io.Writer) int {
	session := os.Getenv("RUSH_SESSION")
	if len(args) >= 2 && args[0] == "--session" {
		session, args = args[1], args[2:]
	}
	if len(args) == 0 || args[0] == "help" || args[0] == "-h" || args[0] == "--help" {
		fmt.Fprint(stdout, remindUsage)
		return 0
	}
	if args[0] == "run" && len(args) == 2 {
		return remindRun(args[1], stderr)
	}
	all := args[0] == "list" && len(args) > 1 && args[1] == "--all"
	if session == "" && !all {
		fmt.Fprintln(stderr, "rush remind: not in a session rush runs; name one with --session <id>")
		return 1
	}
	now := time.Now()
	switch args[0] {
	case "list", "ls":
		remindList(session, all, now, stdout)
		return 0
	case "cancel", "rm":
		return remindCancel(session, args[1:], stdout, stderr)
	}
	if args[0] == "add" {
		args = args[1:]
	}
	if len(args) < 2 {
		fmt.Fprint(stderr, remindUsage)
		return 1
	}
	due, err := remind.Parse(args[0], now)
	if err != nil {
		fmt.Fprintln(stderr, "rush remind:", err)
		return 1
	}
	r, err := remind.Add(session, strings.Join(args[1:], " "), due, now)
	if err != nil {
		fmt.Fprintln(stderr, "rush remind:", err)
		return 1
	}
	fmt.Fprintln(stdout, "set "+remind.Line(r, now))
	return 0
}

// remindList prints session's reminders; every session's when all.
func remindList(session string, all bool, now time.Time, stdout io.Writer) {
	remind.Resume() // a waiter lost to a restart is back before it's listed
	if all {
		session = ""
	}
	list := remind.List(session)
	if len(list) == 0 {
		fmt.Fprintln(stdout, "no reminders")
	}
	for _, r := range list {
		if all {
			fmt.Fprintf(stdout, "%s  · %s\n", remind.Line(r, now), r.Session)
		} else {
			fmt.Fprintln(stdout, remind.Line(r, now))
		}
	}
}

// remindCancel drops one of session's reminders, or all of them.
func remindCancel(session string, args []string, stdout, stderr io.Writer) int {
	if len(args) != 1 {
		fmt.Fprintln(stderr, "usage: rush remind cancel <id>|all")
		return 1
	}
	var err error
	if args[0] == "all" {
		err = remind.CancelAll(session)
	} else {
		err = remind.Cancel(session, args[0])
	}
	if err != nil {
		fmt.Fprintln(stderr, "rush remind:", err)
		return 1
	}
	fmt.Fprintln(stdout, "cancelled")
	return 0
}

// remindRun is the detached waiter remind.Start starts for one reminder.
func remindRun(id string, stderr io.Writer) int {
	release, ok := remind.Hold(id)
	if !ok {
		return 0 // another waiter has it
	}
	defer release()
	err := remind.Waiter{
		Now:   time.Now,
		Sleep: time.Sleep,
		Gone:  func(session string) bool { _, err := host.ReadConfig(session); return err != nil },
		Deliver: func(r remind.Reminder, text string) error {
			b := make([]byte, 16)
			_, _ = rand.Read(b)
			return host.Deliver(r.Session, event.Exchange{ID: hex.EncodeToString(b), Direction: "received", Phase: "message", Text: text,
				Sender: event.Peer{Kind: "rush", Name: "rush · reminder"}}, false)
		},
	}.Run(id)
	if err != nil {
		fmt.Fprintln(stderr, "rush remind:", err)
		return 1
	}
	return 0
}
