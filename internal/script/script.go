// Package script runs an agent's longer shell calls as a file rush can
// follow: each command's line and when it began go to a trace as it runs,
// and a line with a breakpoint stops the script there until it's let go.
//
//	scripts/<id>.sh      the call's command, as written
//	scripts/<id>.trace   "pid N" once, then "LINE TIME" before each command
//	scripts/<id>.breaks  the lines to stop at, one a line
//
// The call is the agent's tool call ID, or, where the harness gives none,
// the command's hash.
package script

import (
	"bufio"
	"crypto/sha256"
	"encoding/hex"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"slices"
	"strconv"
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/gate"
	"github.com/0xdeafcafe/rush/internal/state"
)

// Most is how many commands a call runs as it is; past it, as a script.
const Most = 3

// Dir is where scripts and their traces are kept.
func Dir() string { return filepath.Join(state.Dir(), "scripts") }

// Key is the name a call's files go by: its ID, else its command's hash.
func Key(id, command string) string {
	if id != "" && !strings.ContainsAny(id, `/\.`) {
		return id
	}
	h := sha256.Sum256([]byte(command))
	return hex.EncodeToString(h[:8])
}

// Long is whether command runs more commands than Most.
func Long(command string) bool { return len(gate.Commands(command)) > Most }

// runner is what runs a script: before each command, its line and the
// time go to the trace, and at a line with a breakpoint it stops itself
// once, so rush can let it go on. Sourced, the script's own line numbers
// are the ones $LINENO counts. ponytail: a write per command; a loop of
// thousands writes thousands of lines.
const runner = `__rush_s=$1; __rush_t=${1%.sh}.trace; __rush_b=${1%.sh}.breaks; __rush_at=
printf 'pid %s %s\n' "$$" "$(date +%s)" > "$__rush_t"
__rush_step() {
  [ "${BASH_SOURCE[1]}" = "$__rush_s" ] || return 0
  printf '%s %s\n' "$1" "${EPOCHREALTIME:-$SECONDS}" >> "$__rush_t"
  [ -s "$__rush_b" ] || return 0
  [ "$1" = "$__rush_at" ] && return 0
  __rush_at=$1
  local l
  while IFS= read -r l; do
    if [ "$l" = "$1" ]; then printf 'break %s\n' "$1" >> "$__rush_t"; kill -STOP $$; return 0; fi
  done < "$__rush_b"
}
set -T
trap '__rush_step "$LINENO"' DEBUG
shift
. "$__rush_s"
`

// Bash is a bash new enough to trace a script by its lines (4 or later),
// "" for none: macOS's own is 3.2.
func Bash() string {
	cands := []string{"/opt/homebrew/bin/bash", "/usr/local/bin/bash"}
	if runtime.GOOS != "darwin" {
		cands = append(cands, "/bin/bash", "/usr/bin/bash")
	}
	if p, err := exec.LookPath("bash"); err == nil && p != "/bin/bash" {
		cands = append([]string{p}, cands...)
	}
	for _, p := range cands {
		if st, err := os.Stat(p); err == nil && !st.IsDir() {
			return p
		}
	}
	return ""
}

// Wrap writes command to its script and is the command that runs it,
// from bash, the runner given inline; "" when there's no bash to.
func Wrap(key, command string) (string, error) {
	sh := Bash()
	if sh == "" {
		return "", nil
	}
	dir := Dir()
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return "", err
	}
	p := filepath.Join(dir, key+".sh")
	if err := os.WriteFile(p, []byte(command+"\n"), 0o644); err != nil {
		return "", err
	}
	_ = os.Remove(filepath.Join(dir, key+".trace"))
	return quote(sh) + " --noprofile --norc -c " + quote(runner) + " rush-script " + quote(p), nil
}

// Hit is a command's line beginning at a time.
type Hit struct {
	Line int
	At   time.Time
}

// Run is a script as its trace has it.
type Run struct {
	PID     int
	Start   time.Time
	Hits    []Hit
	Breaks  []int
	HeldAt  int // the line it stopped at for a breakpoint, 0 when it isn't
	Source  string
	Changed time.Time // when the trace last grew
}

// Read is the run of the script key, false when there's none.
func Read(key string) (Run, bool) {
	dir := Dir()
	src, err := os.ReadFile(filepath.Join(dir, key+".sh"))
	if err != nil {
		return Run{}, false
	}
	r := Run{Source: strings.TrimSuffix(string(src), "\n"), Breaks: Breaks(key)}
	f, err := os.Open(filepath.Join(dir, key+".trace"))
	if err != nil {
		return r, true // written, not begun
	}
	defer f.Close()
	if st, err := f.Stat(); err == nil {
		r.Changed = st.ModTime()
	}
	sc := bufio.NewScanner(f)
	var base float64 // what $SECONDS counts from, where the shell has no EPOCHREALTIME
	for sc.Scan() {
		w := strings.Fields(sc.Text())
		if len(w) < 2 {
			continue
		}
		switch w[0] {
		case "pid":
			r.PID, _ = strconv.Atoi(w[1])
			if len(w) > 2 {
				base, _ = strconv.ParseFloat(w[2], 64)
				r.Start = when(base)
			}
		case "break":
			r.HeldAt, _ = strconv.Atoi(w[1])
		default:
			n, err := strconv.Atoi(w[0])
			t, err2 := strconv.ParseFloat(w[1], 64)
			if err != nil || err2 != nil {
				continue
			}
			if !strings.Contains(w[1], ".") {
				t += base
			}
			if r.HeldAt != 0 && n != r.HeldAt {
				r.HeldAt = 0 // let go: it's moved on
			}
			r.Hits = append(r.Hits, Hit{n, when(t)})
		}
	}
	return r, true
}

func when(sec float64) time.Time {
	return time.Unix(0, int64(sec*float64(time.Second)))
}

// Breaks are the lines script key stops at.
func Breaks(key string) []int {
	b, _ := os.ReadFile(filepath.Join(Dir(), key+".breaks"))
	var out []int
	for _, l := range strings.Fields(string(b)) {
		if n, err := strconv.Atoi(l); err == nil {
			out = append(out, n)
		}
	}
	return out
}

// Toggle sets a breakpoint at line of script key, or takes it away, and
// says whether it's set now.
func Toggle(key string, line int) (bool, error) {
	bs := Breaks(key)
	on := !slices.Contains(bs, line)
	if on {
		bs = append(bs, line)
	} else {
		bs = slices.DeleteFunc(bs, func(n int) bool { return n == line })
	}
	slices.Sort(bs)
	var b strings.Builder
	for _, n := range bs {
		b.WriteString(strconv.Itoa(n) + "\n")
	}
	return on, os.WriteFile(filepath.Join(Dir(), key+".breaks"), []byte(b.String()), 0o644)
}

func quote(s string) string { return "'" + strings.ReplaceAll(s, "'", `'\''`) + "'" }
