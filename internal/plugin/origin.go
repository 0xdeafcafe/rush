package plugin

import (
	"context"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/photon/uithread"
)

// --- where a plugin came from ---

// Origin is where an installed plugin came from, as rush saw it when it
// installed it. A plugin put in Root() by hand has none recorded.
type Origin struct {
	Project     string    `json:"project,omitempty"`    // the repository it was installed from
	Remote      string    `json:"remote,omitempty"`     // that repository's origin, without any credentials
	Commit      string    `json:"commit,omitempty"`     // the repository's commit then
	Uncommitted bool      `json:"uncommitted,omitzero"` // its folder there had changes not yet committed
	Version     string    `json:"version,omitempty"`
	Digest      string    `json:"digest,omitempty"` // what the install built, to tell if it changed since
	InstalledAt time.Time `json:"installedAt,omitzero"`

	// Link is where its folder in Root() points, when it's a link to a
	// plugin being worked on. Read, not recorded.
	Link string `json:"-"`
}

func originsPath() string { return filepath.Join(Root(), "origins.json") }

func readOrigins() map[string]Origin {
	out := map[string]Origin{}
	if b, err := os.ReadFile(originsPath()); err == nil {
		_ = jsonx.Unmarshal(b, &out)
	}
	return out
}

// originOf is where the plugin named name, in dir, came from.
func originOf(name, dir string) Origin {
	o := readOrigins()[name]
	if fi, err := os.Lstat(dir); err == nil && fi.Mode()&os.ModeSymlink != 0 {
		if t, err := filepath.EvalSymlinks(dir); err == nil {
			o.Link = t
		}
	}
	return o
}

// recordOrigin notes that the offer was just installed, from its
// repository as it is now.
func recordOrigin(o Offer) error {
	uithread.Forbid("plugin.recordOrigin")
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	git := func(args ...string) string {
		out, err := exec.CommandContext(ctx, "git", append([]string{"-C", o.Project}, args...)...).Output()
		if err != nil {
			return ""
		}
		return strings.TrimSpace(string(out))
	}
	rel, err := filepath.Rel(o.Project, o.Dir)
	if err != nil {
		rel = o.Dir
	}
	d, _ := Digest(filepath.Join(Root(), o.Name))
	all := readOrigins()
	all[o.Name] = Origin{
		Project:     o.Project,
		Remote:      cleanRemote(git("remote", "get-url", "origin")),
		Commit:      git("rev-parse", "HEAD"),
		Uncommitted: git("status", "--porcelain", "--", rel) != "",
		Version:     o.Version,
		Digest:      d,
		InstalledAt: time.Now(),
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
	if c := o.Commit; c != "" {
		s += " at commit " + c[:min(7, len(c))]
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
