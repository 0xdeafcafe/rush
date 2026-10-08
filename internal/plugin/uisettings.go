package plugin

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"syscall"
	"unicode/utf8"

	"github.com/0xdeafcafe/photon/jsonx"
)

// A plugin's settings are kept by rush, in Root(), where the plugin can't
// read or write: it is told them at initialize and when one changes.

// MaxSettingText is the longest a text setting's value may be, in bytes.
const MaxSettingText = 1000

func settingsPath() string { return filepath.Join(Root(), "settings.json") }
func settingsLock() string { return filepath.Join(Root(), "settings.lock") }

// readSettings is every plugin's stored values, by plugin then key.
func readSettings() map[string]map[string]string {
	out := map[string]map[string]string{}
	if b, err := os.ReadFile(settingsPath()); err == nil {
		_ = jsonx.Unmarshal(b, &out)
	}
	return out
}

// SettingValues is the approved plugin's settings: what you set, or each
// one's default. A plugin that isn't approved has none.
func SettingValues(name string) map[string]string {
	a, ok := Approvals()[name]
	if !ok {
		return map[string]string{}
	}
	return SettingValuesOf(&a.Manifest)
}

// SettingValuesOf is SettingValues for a manifest already in hand. A stored
// value its settings no longer allow is taken as unset.
func SettingValuesOf(m *Manifest) map[string]string {
	out := make(map[string]string, len(m.Settings))
	if len(m.Settings) == 0 {
		return out
	}
	stored := readSettings()[m.Name]
	for _, s := range m.Settings {
		v, ok := stored[s.Key]
		if !ok || checkSetting(s, v) != nil {
			v = defaultSetting(s)
		}
		out[s.Key] = v
	}
	return out
}

func defaultSetting(s SettingSpec) string {
	switch {
	case s.Default != "":
		return s.Default
	case s.Type == "bool":
		return "false"
	case s.Type == "choice" && len(s.Choices) > 0:
		return s.Choices[0]
	}
	return ""
}

// checkSetting says whether v is a value setting s allows.
func checkSetting(s SettingSpec, v string) error {
	switch s.Type {
	case "bool":
		if v != "true" && v != "false" {
			return fmt.Errorf("%s is true or false", s.Key)
		}
	case "choice":
		if !slices.Contains(s.Choices, v) {
			return fmt.Errorf("%s is one of %v", s.Key, s.Choices)
		}
	case "text":
		if len(v) > MaxSettingText {
			return fmt.Errorf("%s is at most %d bytes", s.Key, MaxSettingText)
		}
		if !utf8.ValidString(v) || printable(v) != v {
			return fmt.Errorf("%s can't hold control characters", s.Key)
		}
	default:
		return fmt.Errorf("%s has no type rush knows", s.Key)
	}
	return nil
}

// SetSetting keeps a value for one of an approved plugin's settings, if the
// manifest you approved offers it and allows the value.
func SetSetting(name, key, value string) error {
	a, ok := Approvals()[name]
	if b, bundled := BundleNamed(name); bundled {
		// A bundled plugin's name is its own, as in Enabled.
		a, ok = Approval{Manifest: b.Manifest}, true
	}
	if !ok {
		return fmt.Errorf("%s is not approved", name)
	}
	i := slices.IndexFunc(a.Manifest.Settings, func(s SettingSpec) bool { return s.Key == key })
	if i < 0 {
		return fmt.Errorf("%s has no setting %q", name, key)
	}
	if err := checkSetting(a.Manifest.Settings[i], value); err != nil {
		return err
	}
	if err := os.MkdirAll(Root(), 0o700); err != nil {
		return err
	}
	// Two writers (two rushes, say) take turns, so neither loses the other's.
	lock, err := os.OpenFile(settingsLock(), os.O_CREATE|os.O_RDWR, 0o600)
	if err != nil {
		return err
	}
	defer lock.Close()
	if err := syscall.Flock(int(lock.Fd()), syscall.LOCK_EX); err != nil {
		return err
	}
	defer func() { _ = syscall.Flock(int(lock.Fd()), syscall.LOCK_UN) }()

	all := readSettings()
	if all[name] == nil {
		all[name] = map[string]string{}
	}
	all[name][key] = value
	b, err := jsonx.MarshalIndent(all)
	if err != nil {
		return err
	}
	f, err := os.CreateTemp(Root(), ".settings-*.tmp")
	if err != nil {
		return err
	}
	_, werr := f.Write(b)
	if cerr := f.Close(); werr == nil {
		werr = cerr
	}
	if werr == nil {
		werr = os.Chmod(f.Name(), 0o600)
	}
	if werr == nil {
		werr = os.Rename(f.Name(), settingsPath())
	}
	if werr != nil {
		_ = os.Remove(f.Name())
		return errors.Join(errors.New("saving settings"), werr)
	}
	return nil
}
