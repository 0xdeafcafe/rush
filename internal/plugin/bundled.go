package plugin

import (
	"errors"
	"io"
	"os"
	"path/filepath"
	"slices"
	"sort"
	"sync"

	"github.com/0xdeafcafe/rush/internal/jsonx"
	"github.com/0xdeafcafe/rush/internal/uithread"
)

// A bundled plugin ships inside rush. Its code is rush's own, so it's
// trusted as rush is: it runs without being approved, outside the sandbox,
// on every system, as `rush plugin run <name>` speaking the same protocol
// on fd 3 as any other. What it may do in rush's screen is still its
// manifest's, checked by the broker as for any plugin. Each is on until
// you turn it off, unless it's Optional.
type Bundle struct {
	Manifest Manifest
	// Optional is off until you turn it on.
	Optional bool
	// Run is its main, given its end of the connection to the broker.
	Run func(rw io.ReadWriteCloser) error
}

// bundledDigest stands for a bundled plugin's files: they're rush's
// binary, which the broker runs from.
const bundledDigest = "bundled"

var (
	bundleMu sync.RWMutex
	bundles  = map[string]Bundle{}
)

// RegisterBundle makes a bundled plugin known. Its package calls it from
// init; a test may call the func it returns to forget it again.
func RegisterBundle(b Bundle) (forget func()) {
	if err := b.Manifest.validateBundled(); err != nil {
		panic("bundled plugin " + b.Manifest.Name + ": " + err.Error())
	}
	bundleMu.Lock()
	defer bundleMu.Unlock()
	bundles[b.Manifest.Name] = b
	return func() {
		bundleMu.Lock()
		defer bundleMu.Unlock()
		delete(bundles, b.Manifest.Name)
	}
}

func (m *Manifest) validateBundled() error {
	if !nameRE.MatchString(m.Name) {
		return errors.New("bad name")
	}
	// It reaches no network through the proxy: it has the machine's. A
	// program it runs through exec gets your environment, as any plugin's.
	if m.Proto() != ProtoRush || len(m.Network) > 0 {
		return errors.New("a bundled plugin speaks rush's protocol, with no network")
	}
	if err := m.validateExec(); err != nil {
		return err
	}
	if err := m.validateUI(); err != nil {
		return err
	}
	return m.validateCLI()
}

// Bundles are the bundled plugins, by name.
func Bundles() []Bundle {
	bundleMu.RLock()
	defer bundleMu.RUnlock()
	out := make([]Bundle, 0, len(bundles))
	for _, b := range bundles {
		out = append(out, b)
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Manifest.Name < out[j].Manifest.Name })
	return out
}

// BundleNamed is the bundled plugin called name.
func BundleNamed(name string) (Bundle, bool) {
	bundleMu.RLock()
	defer bundleMu.RUnlock()
	b, ok := bundles[name]
	return b, ok
}

func offPath() string { return filepath.Join(Root(), "bundled-off.json") }

// onPath lists the Optional ones you turned on.
func onPath() string { return filepath.Join(Root(), "bundled-on.json") }

func readList(path string) []string {
	var out []string
	if b, err := os.ReadFile(path); err == nil {
		_ = jsonx.Unmarshal(b, &out)
	}
	return out
}

func writeList(path string, list []string) error {
	sort.Strings(list)
	if err := os.MkdirAll(Root(), 0o700); err != nil {
		return err
	}
	b, err := jsonx.Marshal(list)
	if err != nil {
		return err
	}
	tmp := path + ".tmp"
	if err := os.WriteFile(tmp, b, 0o600); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}

// BundledOn says whether you have a bundled plugin on. It runs only if
// its manifest's requirements are met too: see Unmet.
func BundledOn(name string) bool {
	b, ok := BundleNamed(name)
	return ok && bundledOn(&b, readList(offPath()), readList(onPath()))
}

func bundledOn(b *Bundle, off, on []string) bool {
	if b.Optional {
		return slices.Contains(on, b.Manifest.Name)
	}
	return !slices.Contains(off, b.Manifest.Name)
}

// BundlesOn says, by name, which bundled plugins you have on. It reads
// the disk, so it's not for the UI goroutine.
func BundlesOn() map[string]bool {
	uithread.Forbid("plugin.BundlesOn")
	off, on := readList(offPath()), readList(onPath())
	out := map[string]bool{}
	bs := Bundles()
	for i := range bs {
		out[bs[i].Manifest.Name] = bundledOn(&bs[i], off, on)
	}
	return out
}

// SetBundled turns a bundled plugin on or off.
func SetBundled(name string, on bool) error {
	uithread.Forbid("plugin.SetBundled")
	b, ok := BundleNamed(name)
	if !ok {
		return errors.New(name + " isn't bundled with rush")
	}
	path, add := offPath(), !on
	if b.Optional {
		path, add = onPath(), on
	}
	list := slices.DeleteFunc(readList(path), func(n string) bool { return n == name })
	if add {
		list = append(list, name)
	}
	return writeList(path, list)
}

// Enabled is every plugin that runs, by name: the approved ones, and the
// bundled ones turned on, less any whose requirements aren't met here. A
// bundled plugin's name is its own: an installed plugin of the same name
// doesn't run.
func Enabled() map[string]Approval {
	out := Approvals()
	for name, a := range out {
		if a.Manifest.Unmet() != "" {
			delete(out, name)
		}
	}
	off, on := readList(offPath()), readList(onPath())
	for _, b := range Bundles() {
		delete(out, b.Manifest.Name)
		if bundledOn(&b, off, on) && b.Manifest.Unmet() == "" {
			out[b.Manifest.Name] = Approval{Digest: bundledDigest, Manifest: b.Manifest}
		}
	}
	return out
}

// RunBundled runs the bundled plugin name on fd 3, as the broker starts it.
func RunBundled(name string) error {
	b, ok := BundleNamed(name)
	if !ok {
		return errors.New(name + " isn't bundled with rush")
	}
	f := os.NewFile(3, "plugin-ipc")
	if f == nil {
		return errors.New("no connection on fd 3")
	}
	return b.Run(f)
}
