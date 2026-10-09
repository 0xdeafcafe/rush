package host

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"

)

// TempRoot holds sessions' scratch folders: one folder per project, one
// per session in it (tmp/<project>/<id>), so what's left behind says whose
// it is and what it was working on.
func TempRoot() string { return under(&tempMemo, "tmp") }

// TempDir is where a session's Claude Code and everything it runs keep
// their scratch files. The session's own folder links to it (tmp); one
// from before the per-project layout has the folder itself there.
func TempDir(id string) string {
	p := filepath.Join(dir(id), "tmp")
	if r, err := os.Readlink(p); err == nil {
		return r
	}
	return p
}

// IsTempDir is whether p is a session's scratch folder as rush lays them
// out: the only folders its tidy-ups empty.
func IsTempDir(p string) bool {
	p = filepath.Clean(p)
	return filepath.Base(p) == "tmp" || filepath.Dir(filepath.Dir(p)) == TempRoot()
}

// IsTemp is whether p is in a temp folder, a session's or the system's:
// somewhere work is scratch, never a project.
func IsTemp(p string) bool {
	if p == "" {
		return false
	}
	// Prefixes, not filepath.Rel: every agent's folder is asked each frame.
	p = filepath.Clean(p)
	if inside(TempRoot(), p) || systemTemp(p) {
		return true
	}
	root := Root()
	if !inside(root, p) || p == root {
		return false
	}
	_, rest, _ := strings.Cut(p[len(root)+1:], string(filepath.Separator))
	return rest == "tmp" || strings.HasPrefix(rest, "tmp"+string(filepath.Separator))
}

// inside is whether clean path p is root or under it, without allocating.
func inside(root, p string) bool {
	return strings.HasPrefix(p, root) && (len(p) == len(root) || p[len(root)] == filepath.Separator)
}

// systemTemp is whether p is in the system's temp folders.
var systemTemp = func(p string) bool {
	return strings.Contains(p, "/var/folders/") || strings.HasPrefix(p, "/tmp/") || strings.HasPrefix(p, "/private/tmp/")
}

// placeTemp is session id's scratch folder for work in cwd: under cwd's
// project, moved there with all it holds from wherever it was before, when
// nothing has it open. A cwd that's itself scratch keeps it where it is.
func placeTemp(id, cwd string) string {
	was := TempDir(id)
	proj := projectOf(cwd)
	if proj == "" || within(was, cwd) {
		_ = os.MkdirAll(was, 0o700)
		return was
	}
	want := filepath.Join(ProjectTemp(proj), id)
	if was == want {
		_ = os.MkdirAll(want, 0o700)
		return want
	}
	if _, err := os.Stat(was); err == nil {
		// Something running out of it would lose its files mid-way.
		if held, ok := heldUnder(was); !ok || len(held) > 0 {
			return was
		}
		if _, err := os.Stat(want); err == nil || os.MkdirAll(filepath.Dir(want), 0o700) != nil || os.Rename(was, want) != nil {
			return was
		}
	}
	if os.MkdirAll(want, 0o700) != nil {
		return was
	}
	link := filepath.Join(dir(id), "tmp")
	_ = os.Remove(link) // the old link; an old folder there was just moved
	_ = os.Symlink(want, link)
	return want
}

// ProjectTemp holds the scratch folders of the sessions working in project.
func ProjectTemp(project string) string { return filepath.Join(TempRoot(), slug(project)) }

// projectOf is the project cwd is in: its repository's main checkout, so
// a worktree's scratch sits with the rest, or cwd when it isn't one.
func projectOf(cwd string) string {
	if cwd == "" || IsTemp(cwd) {
		return ""
	}
	out, err := exec.Command("git", "-C", cwd, "rev-parse", "--path-format=absolute", "--git-common-dir").Output()
	if common := strings.TrimSpace(string(out)); err == nil && filepath.Base(common) == ".git" {
		return filepath.Dir(common)
	}
	return cwd
}

// slug names a project's folder by its whole path, as Claude Code names
// its projects' transcripts: /Users/me/src/app is -Users-me-src-app.
func slug(p string) string {
	return strings.Map(func(r rune) rune {
		if r >= 'a' && r <= 'z' || r >= 'A' && r <= 'Z' || r >= '0' && r <= '9' {
			return r
		}
		return '-'
	}, p)
}

func within(root, p string) bool {
	rel, err := filepath.Rel(root, p)
	return err == nil && rel != ".." && !strings.HasPrefix(rel, ".."+string(filepath.Separator))
}
