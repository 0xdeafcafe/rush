package agent

import (
	"bufio"
	"io"
	"strconv"
	"strings"
	"time"
)

// Memory is what an agent reads for a session, beyond the conversation:
// its instruction files (CLAUDE.md, AGENTS.md), its own memory, settings
// and the like, and what's untidy about them.
type Memory struct {
	// Files are grouped, in Groups' order.
	Files  []MemoryFile
	Groups []string
	// When is when each group's files reach the model.
	When     map[string]string
	Problems []MemoryProblem
	// Dir is the folder its own memory notes are kept in, "" when it keeps
	// none.
	Dir string
}

// MemoryFile is one file an agent reads, or one it would read if it
// existed.
type MemoryFile struct {
	Group   string
	Path    string
	Name    string
	About   string
	Kind    string // a memory note's type: user, feedback, project, reference
	Missing bool
	Size    int64
	Mod     time.Time
	// Up is the tokens it costs every session, roughly; Later, the tokens
	// it costs when it's loaded, which When says.
	Up    int64
	Later int64
	When  string
	Warn  string // what's untidy about it
	Sub   bool   // imported by the file above it
	// Reads is how many times sessions opened a memory note lately, and
	// LastRead when last; Counted, whether they were counted at all.
	// MEMORY.md's line for it reaches every session either way.
	Reads    int
	LastRead time.Time
	Counted  bool
	// Review is why it's worth a look, as rush judges it when it reads it.
	Review string
	// Same is the note it repeats, to merge it into.
	Same string
}

// MemoryProblem is something untidy, and what to do about it.
type MemoryProblem struct {
	Kind  string
	Title string
	Fix   string
	Path  string   // the file to open for it
	Files []string // the files it's about
}

// MemoryReader is an agent that says what it reads for a session of p's
// in cwd, whose transcript (if it has one yet) is at transcript.
type MemoryReader interface {
	Memory(p Profile, cwd, transcript string) Memory
	// ForgetFile deletes one of its files, and whatever points to it.
	ForgetFile(path string) error
}

// MemoryNote is a memory note to write.
type MemoryNote struct {
	Name        string // short-kebab-case, its file's name too
	Title       string // its line in the index
	Description string // what recall picks it by
	Type        string // user, feedback, project or reference
	Body        string
}

// MemoryKeeper is an agent whose memory notes rush can write and tidy.
type MemoryKeeper interface {
	// Remember writes a note into the memory of p's sessions in cwd, and
	// says where.
	Remember(p Profile, cwd string, n MemoryNote) (string, error)
	// TidyMemory fixes what needs no judgement in the memory folder dir,
	// and says what it did.
	TidyMemory(dir string) ([]string, error)
	// MergeNotes puts the note at drop into keep, and deletes drop.
	MergeNotes(keep, drop string) error
	// MakeGlobal moves a project's note into what every project of p's
	// reads.
	MakeGlobal(p Profile, path string) error
	// ReadNote reads a note: its name, description, type and text.
	ReadNote(path string) (MemoryNote, error)
	// CountOpens fills in how often sessions opened each of mem's notes
	// lately: it reads transcripts, so it's asked for only where shown.
	CountOpens(p Profile, mem *Memory)
}

// FrontMatter reads name, description and type from a markdown note's
// frontmatter, at any depth (a memory note keeps its type under
// metadata), and "paths" when a rule has a paths: list.
func FrontMatter(r io.Reader) map[string]string {
	out := map[string]string{}
	sc := bufio.NewScanner(r)
	sc.Buffer(make([]byte, 0, 64<<10), 1<<20)
	block := "" // the key whose value is a block (| or >) on the lines below
	for n := 0; sc.Scan() && n < 60; n++ {
		line := sc.Text()
		if block != "" {
			if t := strings.TrimSpace(line); t != "" && (line[0] == ' ' || line[0] == '\t') {
				out[block] = strings.TrimSpace(out[block] + " " + t)
				continue
			}
			block = ""
		}
		if strings.TrimSpace(line) == "---" {
			if n == 0 {
				continue
			}
			break
		}
		if n == 0 {
			return out // no frontmatter
		}
		k, v, ok := strings.Cut(strings.TrimSpace(line), ":")
		if !ok {
			continue
		}
		switch k = strings.TrimSpace(k); k {
		case "name", "description", "type":
			if out[k] != "" {
				break
			}
			switch v = strings.TrimSpace(v); v {
			case "|", ">", "|-", ">-", "|+", ">+":
				block = k
			default:
				if len(v) > 1 && v[0] == '"' {
					if s, err := strconv.Unquote(v); err == nil {
						out[k] = s
						break
					}
				}
				out[k] = strings.Trim(v, `"'`)
			}
		case "paths", "globs":
			out["paths"] = "yes"
		}
	}
	return out
}
