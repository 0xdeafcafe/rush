package plugin

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"io/fs"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/photon/uithread"
)

// --- where a plugin came from, and keeping it at a version ---

// Origin is where an installed plugin came from, as rush saw it when it
// installed it. A plugin put in Root() by hand has none recorded.
//
// A plugin a repository ships stays at the commit it was installed at:
// InProjects offers an update only once a newer commit changes its
// sources (its folder there and its manifest's builtFrom). In development
// mode it doesn't wait for a commit: the broker installs it again from the
// repository as it is whenever a file in its sources changes.
type Origin struct {
	Project     string    `json:"project,omitempty"`    // the repository it was installed from
	Remote      string    `json:"remote,omitempty"`     // that repository's origin, without any credentials
	Commit      string    `json:"commit,omitempty"`     // the repository's commit then
	Uncommitted bool      `json:"uncommitted,omitzero"` // its sources there had changes not yet committed
	Version     string    `json:"version,omitempty"`
	Digest      string    `json:"digest,omitempty"` // what the install built, to tell if it changed since
	InstalledAt time.Time `json:"installedAt,omitzero"`

	// Sources are its folder in the repository, then its builtFrom, as
	// paths in the repository.
	Sources []string `json:"sources,omitempty"`
	// Tree is git's hash of the sources at Commit: what a newer commit
	// must change to be offered as an update.
	Tree string `json:"tree,omitempty"`
	// Dev is development mode: rebuilt as its sources change.
	Dev bool `json:"dev,omitzero"`
	// Files is a quick hash of the sources' files as installed (names,
	// sizes, times), for development mode to see them change.
	Files string `json:"files,omitempty"`

	// Link is where its folder in Root() points, when it's a link to a
	// plugin being worked on. Read, not recorded.
	Link string `json:"-"`
}

func originsPath() string { return filepath.Join(Root(), "origins.json") }

// originsMu keeps the UI's, the CLI's and the broker's writes in one
// process from losing each other's.
var originsMu sync.Mutex

// Origins reads every recorded origin, by plugin name.
func Origins() map[string]Origin {
	out := map[string]Origin{}
	if b, err := os.ReadFile(originsPath()); err == nil {
		_ = jsonx.Unmarshal(b, &out)
	}
	return out
}

func updateOrigins(f func(map[string]Origin) error) error {
	originsMu.Lock()
	defer originsMu.Unlock()
	all := Origins()
	if err := f(all); err != nil {
		return err
	}
	if err := os.MkdirAll(Root(), 0o700); err != nil {
		return err
	}
	b, err := jsonx.MarshalIndent(all)
	if err != nil {
		return err
	}
	tmp := originsPath() + ".tmp"
	if err := os.WriteFile(tmp, b, 0o600); err != nil {
		return err
	}
	return os.Rename(tmp, originsPath())
}

// originOf is where the plugin named name, in dir, came from.
func originOf(name, dir string) Origin {
	o := Origins()[name]
	if fi, err := os.Lstat(dir); err == nil && fi.Mode()&os.ModeSymlink != 0 {
		if t, err := filepath.EvalSymlinks(dir); err == nil {
			o.Link = t
		}
	}
	return o
}

// git runs git in root, "" when it fails.
func git(root string, args ...string) string {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	out, err := exec.CommandContext(ctx, "git", append([]string{"-C", root}, args...)...).Output()
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(out))
}

// sources are what the plugin in dir, in the repository at root, is built
// from, as paths in the repository: dir, then builtFrom. One that leaves
// the repository is left out.
func sources(root, dir string, builtFrom []string) []string {
	rel, err := filepath.Rel(root, dir)
	if err != nil {
		return nil
	}
	out := []string{rel}
	for _, b := range builtFrom {
		p := filepath.Clean(filepath.Join(rel, b))
		if p == ".." || strings.HasPrefix(p, "../") || filepath.IsAbs(p) {
			continue
		}
		out = append(out, p)
	}
	return out
}

// tree is git's hash of rels at rev, together: "" when one isn't there.
func tree(root, rev string, rels []string) string {
	args := []string{"rev-parse"}
	for _, r := range rels {
		args = append(args, rev+":"+filepath.ToSlash(r))
	}
	return git(root, args...)
}

// filesHash is a quick hash of the files under rels in root: names,
// sizes, modes and times, not contents, so looking every few seconds stays
// cheap. Folders git and package managers fill are passed over.
func filesHash(root string, rels []string) string {
	h := sha256.New()
	n := 0
	for _, rel := range rels {
		base := filepath.Join(root, rel)
		_ = filepath.WalkDir(base, func(path string, d fs.DirEntry, err error) error {
			if err != nil {
				return nil
			}
			if d.IsDir() && path != base && (d.Name() == ".git" || d.Name() == "node_modules") {
				return filepath.SkipDir
			}
			if n++; n > 50000 {
				return filepath.SkipAll
			}
			info, err := d.Info()
			if err != nil {
				return nil
			}
			r, _ := filepath.Rel(root, path)
			fmt.Fprintf(h, "%q %o %d %d\n", r, info.Mode(), info.Size(), info.ModTime().UnixNano())
			return nil
		})
	}
	return hex.EncodeToString(h.Sum(nil))
}

// recordOrigin notes that the offer was just installed, from its
// repository as it is now. Development mode, if it was on, stays on.
func recordOrigin(o Offer) error {
	uithread.Forbid("plugin.recordOrigin")
	rels := sources(o.Project, o.Dir, o.BuiltFrom)
	d, _ := Digest(filepath.Join(Root(), o.Name))
	status := git(o.Project, append([]string{"status", "--porcelain", "--"}, rels...)...)
	rec := Origin{
		Project:     o.Project,
		Remote:      cleanRemote(git(o.Project, "remote", "get-url", "origin")),
		Commit:      git(o.Project, "rev-parse", "HEAD"),
		Uncommitted: status != "",
		Version:     o.Version,
		Digest:      d,
		InstalledAt: time.Now(),
		Sources:     rels,
		Tree:        tree(o.Project, "HEAD", rels),
		Files:       filesHash(o.Project, rels),
	}
	return updateOrigins(func(all map[string]Origin) error {
		rec.Dev = all[o.Name].Dev
		all[o.Name] = rec
		return nil
	})
}

// newer says whether the repository at root has a newer commit of what the
// installed plugin was built from: its sources differ from those installed,
// and the commit installed is behind root's. A checkout behind it, or on
// a branch apart, isn't offered, so worktrees on older branches don't
// offer to undo an update. It's the commit there, the one installed, and
// how many commits since changed the sources.
func (o Origin) newer(root string, rels []string) (head string, commits int, ok bool) {
	if o.Tree == "" || o.Commit == "" {
		return "", 0, false
	}
	t := tree(root, "HEAD", rels)
	if t == "" || t == o.Tree {
		return "", 0, false
	}
	if exec.Command("git", "-C", root, "merge-base", "--is-ancestor", o.Commit, "HEAD").Run() != nil {
		return "", 0, false
	}
	n, _ := strconv.Atoi(git(root, append([]string{"rev-list", "--count", o.Commit + "..HEAD", "--"}, rels...)...))
	return git(root, "rev-parse", "HEAD"), n, true
}

// SetDev turns development mode on or off for an installed plugin a
// repository ships. On, the broker installs it again from the repository
// as it is now, without waiting for a commit, and again whenever its files
// change.
func SetDev(name string, on bool) error {
	uithread.Forbid("plugin.SetDev")
	return updateOrigins(func(all map[string]Origin) error {
		o, ok := all[name]
		if !ok || o.Project == "" {
			return fmt.Errorf("%s wasn't installed from a repository: development mode rebuilds it from one", name)
		}
		o.Dev = on
		if on {
			o.Files = "" // build it from the files as they are now
		}
		all[name] = o
		return nil
	})
}

// DevChanged lists the plugins in development mode whose sources' files
// changed since they were last installed, with the hash they have now.
func DevChanged() map[string]string {
	uithread.Forbid("plugin.DevChanged")
	out := map[string]string{}
	for name, o := range Origins() {
		if !o.Dev || o.Project == "" {
			continue
		}
		if h := filesHash(o.Project, o.Sources); h != o.Files {
			out[name] = h
		}
	}
	return out
}

// Rebuild installs a plugin in development mode again from its repository
// as it is, and approves what it built if it asks for nothing more than
// was approved: a change to what it may do still waits on you. It says
// whether it approved it.
func Rebuild(name string) (bool, error) {
	uithread.Forbid("plugin.Rebuild")
	o, ok := Origins()[name]
	if !ok || !o.Dev || len(o.Sources) == 0 {
		return false, fmt.Errorf("%s isn't in development mode", name)
	}
	dir := filepath.Join(o.Project, o.Sources[0])
	var m Manifest
	b, err := os.ReadFile(filepath.Join(dir, "plugin.json"))
	if err != nil {
		return false, err
	}
	if err := jsonx.Unmarshal(b, &m); err != nil {
		return false, fmt.Errorf("plugin.json: %w", err)
	}
	if m.Name != name {
		return false, fmt.Errorf("%s: plugin.json names %q", dir, m.Name)
	}
	if err := Install(Offer{Name: name, Version: m.Version, Dir: dir, Project: o.Project, BuiltFrom: m.BuiltFrom}); err != nil {
		return false, err
	}
	p, err := Load(filepath.Join(Root(), name))
	if err != nil {
		return false, err
	}
	a, ok := Approvals()[name]
	if !ok || !sameAsks(a.Manifest, p.Manifest) {
		return false, nil
	}
	return true, Approve(p)
}

// sameAsks says whether two manifests ask for the same: only their
// description and version may differ.
func sameAsks(a, b Manifest) bool {
	a.Description, a.Version = "", ""
	b.Description, b.Version = "", ""
	ja, err1 := jsonx.Marshal(a)
	jb, err2 := jsonx.Marshal(b)
	return errors.Join(err1, err2) == nil && string(ja) == string(jb)
}

// cleanRemote is a git remote as a place, "github.com/owner/repo", with
// any user or token in it dropped.
func cleanRemote(r string) string {
	r = strings.TrimSuffix(r, ".git")
	if u, err := url.Parse(r); err == nil && u.Host != "" {
		return u.Host + u.Path
	}
	// scp-like: git@github.com:owner/repo
	if at := strings.LastIndex(r, "@"); at >= 0 {
		r = r[at+1:]
	}
	return strings.Replace(r, ":", "/", 1)
}

// From says where p came from, in a line: "From ~/Source/haven
// (github.com/acme/haven) at commit 1a2b3c4, on 25 Oct 2026".
func (p Plugin) From() string {
	o := p.Origin
	switch {
	case p.Bundled:
		return "Comes with rush"
	case o.Link != "":
		return "Linked to " + home(o.Link) + ", where it's being worked on"
	case o.Project == "":
		return "Put in " + home(p.Dir) + " by hand: rush doesn't know where it came from"
	}
	s := "From " + home(o.Project)
	if o.Remote != "" {
		s += " (" + printable(o.Remote) + ")"
	}
	if o.Dev {
		return s + ", in development mode: rebuilt as its files change"
	}
	if c := o.Commit; c != "" {
		s += " at commit " + ShortCommit(c)
		if o.Uncommitted {
			s += " plus changes not yet committed"
		}
	}
	if !o.InstalledAt.IsZero() {
		s += ", on " + o.InstalledAt.Format("2 Jan 2006")
	}
	return s
}

// ChangedSinceInstall says whether the files, whose digest is d, aren't
// what the install built.
func (o Origin) ChangedSinceInstall(d string) bool {
	return o.Digest != "" && d != "" && o.Link == "" && d != o.Digest
}

// ShortCommit is a commit as people read it: its first seven characters.
func ShortCommit(commit string) string { return commit[:min(7, len(commit))] }

func home(p string) string {
	if h, err := os.UserHomeDir(); err == nil && h != "" {
		if p == h {
			return "~"
		}
		if strings.HasPrefix(p, h+string(filepath.Separator)) {
			return "~" + p[len(h):]
		}
	}
	return p
}
