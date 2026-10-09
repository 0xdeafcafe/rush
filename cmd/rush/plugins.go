package main

import (
	"bufio"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"github.com/charmbracelet/x/term"

	"github.com/0xdeafcafe/rush/internal/hooks"
	"github.com/0xdeafcafe/rush/internal/plugin"
	"github.com/0xdeafcafe/rush/internal/plugind"
)

const pluginUsage = `rush plugin — plugins, bundled and sandboxed

  rush plugin list            what's bundled, installed, approved and running
  rush plugin check <name>    check a plugin and show what approving it allows
  rush plugin approve <name>  read what a plugin may do, and let it run
  rush plugin revoke <name>   stop it running
  rush plugin off <name>      turn a bundled plugin off
  rush plugin on <name>       and on again
  rush plugin logs <name>     where its output goes
  rush plugin dir             where installed plugins live

A repository can ship plugins in .rush/plugins/<name>, each with an
install.sh that builds it into $RUSH_PLUGIN_DIR: rush offers to install
them when you work in it.

Plugins you install live in %s/<name>, each with a plugin.json.
Bundled plugins come with rush. Most are on until you turn them off; some,
like the clean-up ones, are off until you turn them on.
`

func pluginCmd(args []string) error {
	if len(args) == 0 {
		fmt.Printf(pluginUsage, plugin.Root())
		return nil
	}
	switch args[0] {
	case "list", "ls":
		return pluginList()
	case "run":
		// How the broker starts a bundled plugin; not meant to be run by hand.
		if len(args) != 2 {
			return errors.New("usage: rush plugin run <name>")
		}
		return plugin.RunBundled(args[1])
	case "on", "off":
		if len(args) != 2 {
			return fmt.Errorf("usage: rush plugin %s <name>", args[0])
		}
		if err := plugin.SetBundled(args[1], args[0] == "on"); err != nil {
			return err
		}
		fmt.Printf("%s is %s.\n", args[1], args[0])
		return hooks.Reload()
	case "check":
		if len(args) != 2 {
			return errors.New("usage: rush plugin check <name>")
		}
		p, err := plugin.Load(filepath.Join(plugin.Root(), args[1]))
		if err != nil {
			return err
		}
		fmt.Print(plugin.Describe(p))
		if err := plugin.Supported(); err != nil {
			return err
		}
		if _, err := (plugin.Launch{Plugin: p}).Program(); err != nil {
			return fmt.Errorf("its program: %w", err)
		}
		if why := p.Manifest.Unmet(); why != "" {
			fmt.Printf("\nIt won't run here: it %s.\n", why)
		}
		a, ok := plugin.Approvals()[p.Name]
		switch d, _ := plugin.Digest(p.Dir); {
		case !ok:
			fmt.Printf("\nValid, not approved. To run it: rush plugin approve %s\n", p.Name)
		case d != a.Digest:
			fmt.Printf("\nValid, changed since approval. To run it: rush plugin approve %s\n", p.Name)
		default:
			fmt.Println("\nValid and approved.")
		}
		return nil
	case "approve":
		if len(args) != 2 {
			return errors.New("usage: rush plugin approve <name>")
		}
		return pluginApprove(args[1])
	case "revoke":
		if len(args) != 2 {
			return errors.New("usage: rush plugin revoke <name>")
		}
		if err := plugin.Revoke(args[1]); err != nil {
			return err
		}
		fmt.Printf("%s revoked; it stops now and won't start again.\n", args[1])
		return plugind.Reload()
	case "dir":
		fmt.Println(plugin.Root())
		return nil
	case "logs":
		if len(args) != 2 {
			return errors.New("usage: rush plugin logs <name>")
		}
		fmt.Println(plugin.LogPath(args[1]))
		fmt.Println(plugin.BrokerLog())
		return nil
	}
	fmt.Printf(pluginUsage, plugin.Root())
	return fmt.Errorf("unknown command %q", args[0])
}

func pluginList() error {
	installed, bad := plugin.Installed()
	approvals := plugin.Approvals()
	running := map[string]plugind.Status{}
	if st, err := plugind.Ask(); err == nil {
		for _, s := range st {
			running[s.Name] = s
		}
	}
	for _, b := range plugin.Bundles() {
		status := "bundled, off: rush plugin on " + b.Manifest.Name
		switch {
		case b.Manifest.Unmet() != "":
			status = "bundled, " + b.Manifest.Unmet()
		case plugin.BundledOn(b.Manifest.Name):
			status = "bundled, on"
			if s, ok := running[b.Manifest.Name]; ok {
				status += ", " + s.State
				if s.Error != "" && s.State != "running" {
					status += ": " + s.Error
				}
			}
		}
		fmt.Printf("%-16s %s\n", b.Manifest.Name, status)
		if b.Manifest.Description != "" {
			fmt.Printf("%-16s %s\n", "", b.Manifest.Description)
		}
	}
	if len(installed) == 0 && len(bad) == 0 {
		fmt.Printf("No plugins installed. Put one in %s/<name>.\n", plugin.Root())
	}
	if err := plugin.Supported(); err != nil {
		fmt.Println(err)
	}
	for _, p := range installed {
		_, bundledName := plugin.BundleNamed(p.Name)
		status := "not approved"
		if reason := p.Manifest.Unmet(); reason != "" {
			status = reason
		} else if bundledName {
			status = "not run: a plugin bundled with rush has this name"
		} else if a, ok := approvals[p.Name]; ok {
			status = "approved"
			if d, err := plugin.Digest(p.Dir); err != nil || d != a.Digest {
				status = "changed since approval: approve again to run it"
			} else if s, ok := running[p.Name]; ok {
				status = s.State
				if s.PID > 0 {
					status += fmt.Sprintf(" (pid %d)", s.PID)
				}
				if s.Restarts > 0 {
					status += fmt.Sprintf(", %d restarts", s.Restarts)
				}
				if s.Error != "" && s.State != "running" {
					status += ": " + s.Error
				}
			}
		}
		fmt.Printf("%-16s %s\n", p.Name, status)
		if p.Description != "" {
			fmt.Printf("%-16s %s\n", "", p.Description)
		}
	}
	names := make([]string, 0, len(bad))
	for n := range bad {
		names = append(names, n)
	}
	sort.Strings(names)
	for _, n := range names {
		fmt.Printf("%-16s broken: %v\n", n, bad[n])
	}
	return nil
}

func pluginApprove(name string) error {
	if err := plugin.Supported(); err != nil {
		return err
	}
	p, err := plugin.Load(filepath.Join(plugin.Root(), name))
	if err != nil {
		return err
	}
	fmt.Print(plugin.Describe(p))
	if !term.IsTerminal(os.Stdin.Fd()) {
		return fmt.Errorf("approving needs you at a terminal")
	}
	fmt.Print("\nLet it run? [y/N] ")
	line, _ := bufio.NewReader(os.Stdin).ReadString('\n')
	if a := strings.ToLower(strings.TrimSpace(line)); a != "y" && a != "yes" {
		fmt.Println("Not approved.")
		return nil
	}
	if err := plugin.Approve(p); err != nil {
		return err
	}
	fmt.Printf("Approved. If any file in %s changes, it stops until you approve it again.\n", p.Dir)
	fmt.Println("New sessions get its agents, prompt and tools; running ones get them when Claude Code next starts.")
	return plugind.Reload()
}
