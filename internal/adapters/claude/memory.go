package claude

import (
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/actions"
	"github.com/0xdeafcafe/rush/internal/adapters/claude/claude"
	"github.com/0xdeafcafe/rush/internal/agent"
)

// What Claude Code reads for a session: its auto memory, the CLAUDE.md
// files and rules, settings, and the agents, skills, commands and output
// styles on disk (plugins' are left out; an update would overwrite them).

var groups = []string{"Auto memory", "Instructions", "Settings", "Agents", "Skills", "Commands", "Output styles"}

// when is when each group's files reach Claude.
var when = map[string]string{
	"Auto memory":   "MEMORY.md every session · a note when it's recalled",
	"Instructions":  "every session",
	"Settings":      "never sent to Claude",
	"Agents":        "descriptions every session · the rest when used",
	"Skills":        "descriptions every session · the rest when used",
	"Commands":      "only when you run one",
	"Output styles": "only the one in use",
}

// Memory is what a session of p's in cwd reads. Its auto memory is the
// repository's, kept under its main checkout's folder in p's projects, so
// every worktree of it shares one; out of git, the folder's own, beside
// its transcript.
func (Adapter) Memory(p agent.Profile, cwd, transcript string) agent.Memory {
	proj := projectFolder(p, cwd)
	if proj == "" && transcript != "" {
		proj = filepath.Dir(transcript)
	}
	files, r := memoryFiles(p.Dir, cwd, proj)
	mem := agent.Memory{Files: files, Groups: groups, When: when}
	if proj != "" {
		mem.Dir = filepath.Join(proj, "memory")
		uses := claude.NoteUses(filepath.Join(p.Dir, "projects"), filepath.Base(proj), mem.Dir, readsWindow)
		for i := range mem.Files {
			f := &mem.Files[i]
			if f.Group == "Auto memory" && f.Path != r.Index.Path && !f.Missing {
				u := uses[f.Path]
				f.Reads, f.LastRead, f.Counted = u.Reads, u.Last, true
			}
		}
	}
	for _, pr := range r.Problems {
		mem.Problems = append(mem.Problems, agent.MemoryProblem{Kind: pr.Kind, Title: pr.Title, Fix: pr.Fix, Path: pr.Path, Files: pr.Files})
	}
	return mem
}

// readsWindow is how far back a memory note's reads are counted.
const readsWindow = 60 * 24 * time.Hour

// projectFolder is the folder in p's projects a session in cwd keeps its
// auto memory in: its repository's main checkout's, so worktrees share it.
func projectFolder(p agent.Profile, cwd string) string {
	if cwd == "" || p.Dir == "" {
		return ""
	}
	root := actions.MainRoot(cwd)
	if root == "" {
		root = cwd
	}
	return filepath.Join(p.Dir, "projects", claude.ProjectSlug(root))
}

// Remember writes a memory note for p's sessions in cwd.
func (Adapter) Remember(p agent.Profile, cwd string, n agent.MemoryNote) (string, error) {
	proj := projectFolder(p, cwd)
	if proj == "" {
		return "", fmt.Errorf("no project to remember it in")
	}
	return claude.WriteNote(filepath.Join(proj, "memory"), claude.Note(n))
}

// TidyMemory adds MEMORY.md lines for notes without one, and takes out
// those for notes that don't exist.
func (Adapter) TidyMemory(dir string) ([]string, error) { return claude.FixMemory(dir) }

// MergeNotes puts drop's text below keep's, and deletes drop.
func (Adapter) MergeNotes(keep, drop string) error { return claude.MergeNotes(keep, drop) }

// MakeGlobal moves a note into p's own CLAUDE.md, which every project
// loads.
func (Adapter) MakeGlobal(p agent.Profile, path string) error { return claude.MakeGlobal(p.Dir, path) }

// memoryFiles lists the files, grouped in groups' order, with what each
// costs and when it's loaded, and the checks they came from. The user's
// CLAUDE.md and settings, and the project's CLAUDE.md, are listed even when
// they don't exist yet, so they can be written.
//
//nolint:gocognit,gocyclo,maintidx // one pass over every kind of file Claude Code reads, in the order it shows them
func memoryFiles(cfg, cwd, proj string) ([]agent.MemoryFile, claude.MemReport) {
	r := claude.CheckMemory(cfg, cwd, proj)
	var out []agent.MemoryFile
	seen := map[string]bool{}
	add := func(f agent.MemoryFile, always bool) *agent.MemoryFile {
		if f.Path == "" || seen[f.Path] {
			return nil
		}
		st, err := os.Stat(f.Path)
		if (err != nil || st.IsDir()) && !always {
			return nil
		}
		seen[f.Path] = true
		f.Missing = err != nil
		if err == nil {
			f.Size, f.Mod = st.Size(), st.ModTime()
		}
		out = append(out, f)
		return &out[len(out)-1]
	}
	// The folders a session in cwd reads from, nearest first, up to but not
	// including home, whose .claude is the user's own.
	home, _ := os.UserHomeDir()
	var dirs []string
	for d := cwd; d != "" && d != "/" && d != "." && d != home; d = filepath.Dir(d) {
		dirs = append(dirs, d)
	}
	name := func(p string) string {
		if cwd != "" {
			if rel, err := filepath.Rel(cwd, p); err == nil && !strings.HasPrefix(rel, "..") {
				return rel
			}
		}
		return tilde(p)
	}
	base := func(p string) string { return strings.TrimSuffix(filepath.Base(p), ".md") }
	tok := claude.EstTokens

	if proj != "" {
		ix := r.Index
		if f := add(agent.MemoryFile{Group: "Auto memory", Path: ix.Path, Name: "MEMORY.md", About: "the index", Up: tok(int64(ix.LoadedBytes))}, false); f != nil {
			switch {
			case ix.Cut():
				f.Warn = fmt.Sprintf("only its first %d of %d lines load", ix.LoadedLines, ix.Lines)
			case ix.Long > 0:
				f.Warn = fmt.Sprintf("%d of its lines are over %d characters", ix.Long, claude.IndexLineLen)
			}
		}
		// Notes by kind, as the memory view heads them.
		kind := map[string]int{"user": 0, "feedback": 1, "project": 2, "reference": 3}
		rank := func(t string) int {
			if r, ok := kind[t]; ok {
				return r
			}
			return len(kind)
		}
		sort.SliceStable(r.Notes, func(i, j int) bool { return rank(r.Notes[i].Type) < rank(r.Notes[j].Type) })
		for _, n := range r.Notes {
			f := add(agent.MemoryFile{Group: "Auto memory", Path: n.Path, Name: firstNonEmpty(n.Name, base(n.Path)), About: n.Description,
				Kind: n.Type, Later: tok(n.Size), When: "on recall"}, false)
			if f == nil {
				continue
			}
			switch {
			case n.PastCut:
				f.Warn = "its line in MEMORY.md is past what loads"
			case !n.Indexed && ix.Exists:
				f.Warn = "not in MEMORY.md · found only if recall picks it"
			case n.Same != "":
				f.Warn = "repeats " + base(n.Same)
			case len(n.Gone) > 0:
				f.Warn = "names " + n.Gone[0] + ", which is gone"
			case n.Description == "":
				f.Warn = "no description to recall it by"
			}
		}
		for _, l := range ix.Links {
			if exists(l.File) {
				continue
			}
			if f := add(agent.MemoryFile{Group: "Auto memory", Path: l.File, Name: base(l.File)}, true); f != nil {
				f.Warn = fmt.Sprintf("line %d of MEMORY.md points here · no such note", l.Line)
			}
		}
	}

	// Instructions, each followed by what it imports; the project's own
	// CLAUDE.md goes after the user's, written or not.
	about := map[string]string{"user": "yours · every project", "project": "this project's · shared through git",
		"parent": "a parent folder's", "local": "just yours · kept out of git"}
	if user := filepath.Join(cfg, "CLAUDE.md"); !exists(user) {
		add(agent.MemoryFile{Group: "Instructions", Path: user, Name: tilde(user), About: about["user"]}, true)
	}
	projectMD := func() {
		if cwd != "" && !exists(filepath.Join(cwd, "CLAUDE.md")) && !exists(filepath.Join(cwd, ".claude", "CLAUDE.md")) {
			add(agent.MemoryFile{Group: "Instructions", Path: filepath.Join(cwd, "CLAUDE.md"), Name: "CLAUDE.md", About: about["project"]}, true)
		}
	}
	placed := false
	for _, in := range r.Instr {
		if !placed && in.Scope != "user" && in.Scope != "import" {
			projectMD()
			placed = true
		}
		f := agent.MemoryFile{Group: "Instructions", Path: in.Path, Name: name(in.Path), About: about[in.Scope], Up: tok(in.Size)}
		switch in.Scope {
		case "import":
			f.Sub, f.Name, f.About = true, "@"+in.Ref, "imported by "+filepath.Base(in.From)
			if in.Missing {
				f.Up, f.Warn = 0, "@"+in.Ref+" doesn't lead to a file"
			}
		case "rule":
			f.About = firstNonEmpty(claude.FrontMatter(in.Path)["description"], "rule")
		}
		if in.OnDemand {
			f.Up, f.Later, f.When = 0, tok(in.Size), "with matching files"
		}
		if in.Scope == "user" {
			f.Name = tilde(in.Path)
		}
		add(f, in.Missing)
	}
	if !placed {
		projectMD()
	}

	add(agent.MemoryFile{Group: "Settings", Path: filepath.Join(cfg, "settings.json"), Name: tilde(filepath.Join(cfg, "settings.json")), About: "yours · every project"}, true)
	add(agent.MemoryFile{Group: "Settings", Path: filepath.Join(cfg, "settings.local.json"), Name: tilde(filepath.Join(cfg, "settings.local.json")), About: "yours · this machine"}, false)
	if cwd != "" {
		add(agent.MemoryFile{Group: "Settings", Path: filepath.Join(cwd, ".claude", "settings.json"), Name: filepath.Join(".claude", "settings.json"), About: "project · shared through git"}, false)
		add(agent.MemoryFile{Group: "Settings", Path: filepath.Join(cwd, ".claude", "settings.local.json"), Name: filepath.Join(".claude", "settings.local.json"), About: "project · just yours"}, false)
		add(agent.MemoryFile{Group: "Settings", Path: filepath.Join(cwd, ".mcp.json"), Name: ".mcp.json", About: "project MCP servers"}, false)
	}
	add(agent.MemoryFile{Group: "Settings", Path: claudeJSON(cfg), Name: tilde(claudeJSON(cfg)), About: "Claude Code's own state · MCP servers, trust, per-project history"}, false)

	// An agent's or skill's name and description are listed to Claude every
	// session; the rest when it's used.
	described := func(group, p, name, desc, when string) {
		f := add(agent.MemoryFile{Group: group, Path: p, Name: name, About: desc, When: when}, false)
		if f != nil {
			f.Up = tok(int64(len(name) + len(desc)))
			f.Later = max(0, tok(f.Size)-f.Up)
		}
	}
	for _, root := range append(dotClaude(dirs), cfg) {
		for _, p := range mdTree(filepath.Join(root, "agents")) {
			fm := noteMeta(p)
			described("Agents", p, firstNonEmpty(fm["name"], base(p)), fm["description"], "when used")
		}
		skills, _ := filepath.Glob(filepath.Join(root, "skills", "*", "SKILL.md"))
		for _, p := range skills {
			fm := noteMeta(p)
			described("Skills", p, firstNonEmpty(fm["name"], filepath.Base(filepath.Dir(p))), fm["description"], "when used")
		}
		for _, p := range mdTree(filepath.Join(root, "commands")) {
			rel, _ := filepath.Rel(filepath.Join(root, "commands"), strings.TrimSuffix(p, ".md"))
			if f := add(agent.MemoryFile{Group: "Commands", Path: p, Name: "/" + strings.ReplaceAll(rel, string(filepath.Separator), ":"), About: noteMeta(p)["description"], When: "when run"}, false); f != nil {
				f.Later = tok(f.Size)
			}
		}
		for _, p := range mdTree(filepath.Join(root, "output-styles")) {
			fm := noteMeta(p)
			if f := add(agent.MemoryFile{Group: "Output styles", Path: p, Name: firstNonEmpty(fm["name"], base(p)), About: fm["description"], When: "if chosen"}, false); f != nil {
				f.Later = tok(f.Size)
			}
		}
	}
	// Grouped, keeping each group's own order.
	rank := map[string]int{}
	for i, g := range groups {
		rank[g] = i
	}
	sort.SliceStable(out, func(i, j int) bool { return rank[out[i].Group] < rank[out[j].Group] })
	return out, r
}

// claudeJSON is Claude Code's state file: ~/.claude.json for the default
// config folder, or .claude.json inside any other.
func claudeJSON(cfg string) string {
	if home, _ := os.UserHomeDir(); home != "" && cfg == filepath.Join(home, ".claude") {
		return filepath.Join(home, ".claude.json")
	}
	return filepath.Join(cfg, ".claude.json")
}

func dotClaude(dirs []string) []string {
	out := make([]string, len(dirs))
	for i, d := range dirs {
		out[i] = filepath.Join(d, ".claude")
	}
	return out
}

func exists(p string) bool {
	_, err := os.Stat(p)
	return err == nil
}

// tilde is p with your home folder as ~.
func tilde(p string) string {
	home, _ := os.UserHomeDir()
	if home != "" && strings.HasPrefix(p, home) {
		return "~" + p[len(home):]
	}
	return p
}

// mdTree is every .md file under dir, in order.
func mdTree(dir string) []string {
	var out []string
	_ = filepath.WalkDir(dir, func(p string, d os.DirEntry, err error) error {
		if err == nil && !d.IsDir() && strings.HasSuffix(p, ".md") {
			out = append(out, p)
		}
		return nil
	})
	return out
}

// noteMeta reads name, description and type from a markdown file's
// frontmatter.
func noteMeta(path string) map[string]string { return claude.FrontMatter(path) }

func firstNonEmpty(s ...string) string {
	for _, v := range s {
		if v != "" {
			return v
		}
	}
	return ""
}

// ForgetFile deletes a file, and a memory note's line in its MEMORY.md.
func (Adapter) ForgetFile(path string) error { return claude.ForgetNote(path) }
