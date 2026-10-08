package keymap

import (
	"errors"
	"io/fs"
	"maps"
	"os"
	"path/filepath"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/state"
)

// Path is keybindings.json.
func Path() string { return filepath.Join(state.Dir(), "keybindings.json") }

// Load reads keybindings.json; none is no bindings of yours.
func Load() (File, error) {
	var f File
	b, err := os.ReadFile(Path())
	if errors.Is(err, fs.ErrNotExist) {
		return f, nil
	}
	if err != nil {
		return f, err
	}
	err = jsonx.Unmarshal(b, &f)
	return f, err
}

// Save writes keybindings.json.
func Save(f File) error { return state.WriteJSON(Path(), f) }

// With is f with id's keys set; nil keys go back to the defaults, an empty
// list unbinds it.
func (f File) With(id string, keys []string) File {
	out := File{Bindings: map[string][]string{}}
	maps.Copy(out.Bindings, f.Bindings)
	if keys == nil {
		delete(out.Bindings, id)
	} else {
		out.Bindings[id] = keys
	}
	return out
}
