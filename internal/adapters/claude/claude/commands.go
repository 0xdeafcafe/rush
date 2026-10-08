package claude

import (
	"bufio"
	"os"
	"path/filepath"
	"slices"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
)

type Command = agent.Command

var (
	cmdMu    sync.Mutex
	cmdCache = map[string]cmdEntry{}
)

type cmdEntry struct {
	at   time.Time
	cmds []Command
}

// Commands lists the custom commands and skills a session in cwd can use:
// the account's own, the project's (cwd up to the filesystem root), and
// those of installed and synced plugins, named plugin:name. It is read at
// most every 30 seconds.
func Commands(configDir, cwd string) []Command {
	key := configDir + "\x00" + cwd
	cmdMu.Lock()
	defer cmdMu.Unlock()
	if e, ok := cmdCache[key]; ok && time.Since(e.at) < 30*time.Second {
		return e.cmds
	}
	seen := map[string]bool{}
	var out []Command
	add := func(c Command) {
		if c.Name != "" && !seen[c.Name] {
			seen[c.Name] = true
			out = append(out, c)
		}
	}
	scan := func(root, prefix, source string) {
		for _, c := range readSkills(filepath.Join(root, "skills"), prefix) {
			c.Source = source
			add(c)
		}
		for _, c := range readCommands(filepath.Join(root, "commands"), prefix) {
			c.Source = source
			add(c)
		}
	}
	for d := cwd; d != "" && d != "/" && d != "."; d = filepath.Dir(d) {
		scan(filepath.Join(d, ".claude"), "", "project")
	}
	scan(configDir, "", "yours")
	synced, _ := filepath.Glob(filepath.Join(configDir, "skills", "synced", "*"))
	for _, d := range synced {
		for _, c := range readSkills(d, "") {
			c.Source = "claude.ai"
			add(c)
		}
	}
	for _, root := range pluginRoots(configDir, cwd) {
		name := pluginName(root)
		scan(root, name+":", name)
	}
	sort.SliceStable(out, func(i, j int) bool { return out[i].Name < out[j].Name })
	cmdCache[key] = cmdEntry{at: time.Now(), cmds: out}
	return out
}

// pluginRoots are the folders of plugins installed for this account (user
// scope, or project scope for a project containing cwd) and synced ones.
func pluginRoots(configDir, cwd string) []string {
	var roots []string
	var inst struct {
		Plugins map[string][]struct {
			Scope       string `json:"scope"`
			ProjectPath string `json:"projectPath"`
			InstallPath string `json:"installPath"`
		} `json:"plugins"`
	}
	if b, err := os.ReadFile(filepath.Join(configDir, "plugins", "installed_plugins.json")); err == nil {
		_ = jsonx.Unmarshal(b, &inst)
	}
	for _, list := range inst.Plugins {
		for _, p := range list {
			if p.Scope == "project" && p.ProjectPath != "" && !strings.HasPrefix(cwd+"/", p.ProjectPath+"/") {
				continue
			}
			roots = append(roots, p.InstallPath)
		}
	}
	synced, _ := filepath.Glob(filepath.Join(configDir, "plugins", "synced", "*", "*"))
	for _, d := range synced {
		if st, err := os.Stat(d); err == nil && st.IsDir() {
			roots = append(roots, d)
		}
	}
	return roots
}

// SkillRoots are the folders of <name>/SKILL.md skills Claude Code at
// configDir reads in cwd, less the project's own: the user's, synced ones
// and each plugin's. Another agent reads them for the same skills.
func SkillRoots(configDir, cwd string) []string {
	roots := []string{filepath.Join(configDir, "skills")}
	synced, _ := filepath.Glob(filepath.Join(configDir, "skills", "synced", "*"))
	roots = append(roots, synced...)
	for _, r := range pluginRoots(configDir, cwd) {
		roots = append(roots, filepath.Join(r, "skills"))
	}
	slices.Sort(roots)
	return slices.DeleteFunc(slices.Compact(roots), func(d string) bool {
		st, err := os.Stat(d)
		return err != nil || !st.IsDir()
	})
}

func pluginName(root string) string {
	var m struct {
		Name string `json:"name"`
	}
	if b, err := os.ReadFile(filepath.Join(root, ".claude-plugin", "plugin.json")); err == nil {
		_ = jsonx.Unmarshal(b, &m)
	}
	if m.Name != "" {
		return m.Name
	}
	return filepath.Base(root)
}

func readSkills(dir, prefix string) []Command {
	files, _ := filepath.Glob(filepath.Join(dir, "*", "SKILL.md"))
	var out []Command
	for _, f := range files {
		fm := frontmatter(f)
		name := firstOf(fm["name"], filepath.Base(filepath.Dir(f)))
		out = append(out, Command{Name: prefix + name, Description: fm["description"], ArgumentHint: fm["argument-hint"], Skill: true, Path: f, Size: fileSize(f)})
	}
	return out
}

// readCommands reads commands/*.md, where a subfolder becomes part of the
// name (commands/git/push.md is git:push).
func readCommands(dir, prefix string) []Command {
	var out []Command
	_ = filepath.WalkDir(dir, func(p string, d os.DirEntry, err error) error {
		if err != nil || d.IsDir() || !strings.HasSuffix(p, ".md") {
			return nil
		}
		rel, _ := filepath.Rel(dir, strings.TrimSuffix(p, ".md"))
		fm := frontmatter(p)
		out = append(out, Command{Name: prefix + strings.ReplaceAll(rel, string(filepath.Separator), ":"),
			Description: firstOf(fm["description"], fm[""]), ArgumentHint: fm["argument-hint"], Path: p, Size: fileSize(p)})
		return nil
	})
	return out
}

// frontmatter reads the simple key: value lines between --- fences at the
// top of a markdown file. Without one, "" holds the first line of text.
func frontmatter(path string) map[string]string {
	f, err := os.Open(path)
	if err != nil {
		return nil
	}
	defer f.Close()
	out := map[string]string{}
	sc := bufio.NewScanner(f)
	sc.Buffer(make([]byte, 0, 64<<10), 1<<20)
	in := false
	for n := 0; sc.Scan() && n < 60; n++ {
		line := sc.Text()
		if n == 0 && strings.TrimSpace(line) == "---" {
			in = true
			continue
		}
		if !in {
			if t := strings.TrimSpace(strings.TrimLeft(line, "# ")); t != "" {
				out[""] = t
				return out
			}
			continue
		}
		if strings.TrimSpace(line) == "---" {
			return out
		}
		if k, v, ok := strings.Cut(line, ":"); ok && !strings.HasPrefix(k, " ") {
			out[strings.TrimSpace(k)] = strings.Trim(strings.TrimSpace(v), `"'`)
		}
	}
	return out
}

func firstOf(xs ...string) string {
	for _, x := range xs {
		if x != "" {
			return x
		}
	}
	return ""
}

func fileSize(p string) int64 {
	if st, err := os.Stat(p); err == nil {
		return st.Size()
	}
	return 0
}

// ReadSkills reads a folder of <name>/SKILL.md skills, and ReadCommands a
// folder of markdown commands, for agents that keep theirs as Claude Code
// does.
func ReadSkills(dir, prefix string) []Command   { return readSkills(dir, prefix) }
func ReadCommands(dir, prefix string) []Command { return readCommands(dir, prefix) }
