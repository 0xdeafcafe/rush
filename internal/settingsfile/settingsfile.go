// Package settingsfile edits a JSON settings file in place, as an agent
// reads it live: keys keep their order, keys rush doesn't know are kept
// exactly, and a save applies only the changes made here to the file as
// it is then.
package settingsfile

import (
	"bytes"
	"encoding/json/jsontext"
	"errors"
	"os"
	"path/filepath"
	"slices"
	"strings"

	"github.com/0xdeafcafe/photon/jsonx"
)

// File is a settings file, edited in place. Keys keep their order, keys
// rush doesn't know about are kept exactly, and a save re-reads the file
// and applies only the changes made here, so whatever the agent wrote in
// the meantime survives. It writes a temp file then renames it (through a
// symlink, to the real file), because running sessions read the file live.
type File struct {
	Path    string
	raw     *object
	changes []change
}

type change struct {
	keys []string
	val  jsontext.Value // nil removes
}

// object is a JSON object that remembers its key order.
type object struct {
	keys []string
	vals map[string]jsontext.Value
}

func newObject() *object { return &object{vals: map[string]jsontext.Value{}} }

func parseObject(b []byte) (*object, error) {
	o := newObject()
	if len(bytes.TrimSpace(b)) == 0 {
		return o, nil
	}
	d := jsonx.NewDecoder(bytes.NewReader(b))
	t, err := d.ReadToken()
	if err != nil {
		return nil, err
	}
	if t.Kind() != '{' {
		return nil, errors.New("settings.json isn't a JSON object")
	}
	for d.PeekKind() != '}' {
		t, err := d.ReadToken()
		if err != nil {
			return nil, err
		}
		k := t.String()
		v, err := d.ReadValue()
		if err != nil {
			return nil, err
		}
		v = v.Clone() // the decoder reuses its buffer
		if _, dup := o.vals[k]; !dup {
			o.keys = append(o.keys, k)
		}
		o.vals[k] = v
	}
	return o, nil
}

func (o *object) set(k string, v jsontext.Value) {
	if v == nil {
		if _, ok := o.vals[k]; ok {
			delete(o.vals, k)
			o.keys = slices.DeleteFunc(o.keys, func(x string) bool { return x == k })
		}
		return
	}
	if _, ok := o.vals[k]; !ok {
		o.keys = append(o.keys, k)
	}
	o.vals[k] = v
}

func (o *object) bytes() []byte {
	var b bytes.Buffer
	b.WriteByte('{')
	for i, k := range o.keys {
		if i > 0 {
			b.WriteByte(',')
		}
		b.Write(encode(k))
		b.WriteByte(':')
		b.Write(o.vals[k])
	}
	b.WriteByte('}')
	return b.Bytes()
}

// encode is v in JSON, <, > and & unescaped: a hook command like
// `a && b > out` must stay readable.
func encode(v any) []byte {
	b, err := jsonx.Marshal(v)
	if err != nil {
		return nil
	}
	return b
}

// Load reads a settings file; a missing one is empty.
func Load(path string) (*File, error) {
	s := &File{Path: path}
	raw, err := readObject(s.Path)
	if err != nil {
		return nil, err
	}
	s.raw = raw
	return s, nil
}

func readObject(path string) (*object, error) {
	b, err := os.ReadFile(path)
	if errors.Is(err, os.ErrNotExist) {
		return newObject(), nil
	}
	if err != nil {
		return nil, err
	}
	return parseObject(b)
}

// Get reads a value by dotted path, e.g. "permissions.defaultMode", into v.
// It reports whether the key is set.
func (s *File) Get(path string, v any) bool {
	raw, ok := s.lookup(strings.Split(path, "."))
	if !ok {
		return false
	}
	return jsonx.Unmarshal(raw, v) == nil
}

// String is Get for a string, "" when unset.
func (s *File) String(path string) string {
	var v string
	s.Get(path, &v)
	return v
}

func (s *File) lookup(keys []string) (jsontext.Value, bool) {
	cur := s.raw
	for i, k := range keys {
		raw, ok := cur.vals[k]
		if !ok {
			return nil, false
		}
		if i == len(keys)-1 {
			return raw, true
		}
		next, err := parseObject(raw)
		if err != nil {
			return nil, false
		}
		cur = next
	}
	return nil, false
}

// Set writes a value by dotted path; nil removes the key. Objects on the way
// are created, and their other keys kept in order.
func (s *File) Set(path string, v any) error {
	keys := strings.Split(path, ".")
	var val jsontext.Value
	if v != nil {
		if val = encode(v); val == nil {
			return errors.New("can't write that value")
		}
	}
	setIn(s.raw, keys, val)
	s.changes = append(s.changes, change{keys, val})
	return nil
}

func setIn(o *object, keys []string, val jsontext.Value) {
	k := keys[0]
	if len(keys) == 1 {
		o.set(k, val)
		return
	}
	child := newObject()
	if raw, ok := o.vals[k]; ok {
		if c, err := parseObject(raw); err == nil {
			child = c
		}
	}
	setIn(child, keys[1:], val)
	if len(child.keys) == 0 {
		o.set(k, nil)
		return
	}
	o.set(k, child.bytes())
}

// Env is the settings' env block: variables every session starts with.
func (s *File) Env() map[string]string {
	env := map[string]string{}
	s.Get("env", &env)
	return env
}

// SetEnv sets one variable; an empty value removes it.
func (s *File) SetEnv(name, value string) error {
	if value == "" {
		return s.Set("env."+name, nil)
	}
	return s.Set("env."+name, value)
}

// Detach hands the changes made here so far to a File of their own, to be
// saved on another goroutine while this one is still read and changed: s
// keeps showing them, and a save of it later won't write them again.
func (s *File) Detach() *File {
	d := &File{Path: s.Path, raw: newObject(), changes: s.changes}
	s.changes = nil
	return d
}

// Save applies the changes made here to the file as it is now, indented
// two spaces, as agents write theirs.
func (s *File) Save() error {
	path := s.Path
	if real, err := filepath.EvalSymlinks(path); err == nil {
		path = real // a dotfiles symlink stays a symlink
	}
	cur, err := readObject(path)
	if err != nil {
		return err
	}
	for _, c := range s.changes {
		setIn(cur, c.keys, c.val)
	}
	out, err := jsonx.Indent(cur.bytes())
	if err != nil {
		return err
	}
	b := bytes.NewBuffer(append(out, '\n'))
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return err
	}
	tmp, err := os.CreateTemp(filepath.Dir(path), ".settings-*.json")
	if err != nil {
		return err
	}
	defer os.Remove(tmp.Name())
	if _, err := tmp.Write(b.Bytes()); err != nil {
		tmp.Close()
		return err
	}
	if err := tmp.Close(); err != nil {
		return err
	}
	mode := os.FileMode(0o644)
	if st, err := os.Stat(path); err == nil {
		mode = st.Mode().Perm()
	}
	_ = os.Chmod(tmp.Name(), mode)
	if err := os.Rename(tmp.Name(), path); err != nil {
		return err
	}
	s.raw, s.changes = cur, nil
	return nil
}
