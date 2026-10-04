package state

import (
	"bytes"
	"encoding/json/jsontext"
	"os"

	"github.com/0xdeafcafe/rush/internal/jsonx"
)

// writeConfig puts config b on disk. Something other than this rush
// (another rush, an editor, a program that syncs settings between
// machines) may have changed config.json since this rush last read it,
// and this rush hasn't looked yet: each setting that one changed and this
// rush didn't is written as the file has it, not as this rush remembers
// it. The file is then left to show as changed, so the next look at it
// (ConfigOnDisk, Reload) takes those settings in.
func writeConfig(path string, b []byte) error {
	seenConfig.Lock()
	base := seenConfig.b
	seenConfig.Unlock()
	disk, err := os.ReadFile(path)
	if err != nil || base == nil || bytes.Equal(disk, base) {
		return writeBytes(path, b)
	}
	merged, ok := mergeOutside(base, disk, b)
	if !ok {
		return writeBytes(path, b)
	}
	if err := writeBytes(path, merged); err != nil {
		return err
	}
	noteConfig(path, base)
	return nil
}

// mergeOutside is config mine with each top-level setting that disk
// changed from base, and mine didn't, as disk has it. It's false when
// there is none, or one of the three can't be read.
func mergeOutside(base, disk, mine []byte) ([]byte, bool) {
	var was, now, out map[string]jsontext.Value
	if jsonx.Unmarshal(base, &was) != nil || jsonx.Unmarshal(disk, &now) != nil || jsonx.Unmarshal(mine, &out) != nil {
		return nil, false
	}
	if out == nil {
		out = map[string]jsontext.Value{}
	}
	took := false
	take := func(key string) {
		if sameValue(now, was, key) || !sameValue(out, was, key) {
			return
		}
		if v, ok := now[key]; ok {
			out[key] = v
		} else {
			delete(out, key)
		}
		took = true
	}
	for key := range now {
		take(key)
	}
	for key := range was {
		take(key)
	}
	if !took {
		return nil, false
	}
	// Through Config, so the file keeps the order and form rush writes.
	b, err := jsonx.Marshal(out)
	if err != nil {
		return nil, false
	}
	var c Config
	if jsonx.Unmarshal(b, &c) != nil {
		return nil, false
	}
	merged, err := jsonx.MarshalIndent(c)
	if err != nil {
		return nil, false
	}
	return merged, true
}

// sameValue is whether a and b hold the same value at key, or both none,
// however each is spaced.
func sameValue(a, b map[string]jsontext.Value, key string) bool {
	x, inA := a[key]
	y, inB := b[key]
	if inA != inB {
		return false
	}
	if !inA {
		return true
	}
	x, y = bytes.Clone(x), bytes.Clone(y)
	if x.Compact() != nil || y.Compact() != nil {
		return false
	}
	return bytes.Equal(x, y)
}
