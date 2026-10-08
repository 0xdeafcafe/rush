package host

import (
	"bufio"
	"bytes"
	"os"
	"path/filepath"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent/event"
)

const exchangeJournalBytes = 8 << 20
const exchangeJournalRecords = 2048

// ReadExchanges reads only this Rush session's bounded exchange journal. Call
// from a host or background history reader, never a UI render/update loop.
func ReadExchanges(id string) []event.Exchange {
	f, err := os.Open(filepath.Join(dir(id), "exchanges.jsonl"))
	if err != nil {
		return nil
	}
	defer f.Close()
	stat, err := f.Stat()
	if err != nil {
		return nil
	}
	offset := max(int64(0), stat.Size()-exchangeJournalBytes)
	if offset > 0 {
		f.Seek(offset, 0)
	}
	r := bufio.NewScanner(f)
	r.Buffer(make([]byte, 4096), exchangeJournalBytes)
	if offset > 0 {
		r.Scan()
	} // partial oldest entry
	var out []event.Exchange
	for r.Scan() {
		var e event.Exchange
		if jsonx.Unmarshal(r.Bytes(), &e) == nil && e.ID != "" {
			e.Archived = true
			out = append(out, e)
		}
	}
	if len(out) > exchangeJournalRecords {
		out = out[len(out)-exchangeJournalRecords:]
	}
	return out
}

// appendExchange runs under the owning host's mutex. Entries are appended once;
// occasional atomic compaction bounds storage without truncating an entry.
func appendExchange(id string, e event.Exchange) error {
	if id == "" || e.ID == "" {
		return nil
	}
	p := filepath.Join(dir(id), "exchanges.jsonl")
	b, err := jsonx.Marshal(e)
	if err != nil {
		return err
	}
	// Individual messages larger than the journal budget retain their normal
	// transcript; do not make every future attachment load unbounded.
	if len(b) > exchangeJournalBytes {
		return nil
	}
	if stat, err := os.Stat(p); err == nil && stat.Size()+int64(len(b)+2) > exchangeJournalBytes {
		records := ReadExchanges(id)
		var kept [][]byte
		size := len(b) + 1
		for i := len(records) - 1; i >= 0; i-- {
			records[i].Archived = false
			line, _ := jsonx.Marshal(records[i])
			if size+len(line)+1 > exchangeJournalBytes {
				break
			}
			kept = append(kept, line)
			size += len(line) + 1
		}
		var out bytes.Buffer
		for i := len(kept) - 1; i >= 0; i-- {
			out.Write(kept[i])
			out.WriteByte('\n')
		}
		out.Write(b)
		out.WriteByte('\n')
		tmp := p + ".tmp"
		if err := os.WriteFile(tmp, out.Bytes(), 0600); err != nil {
			return err
		}
		return os.Rename(tmp, p)
	}
	f, err := os.OpenFile(p, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0600)
	if err != nil {
		return err
	}
	_, err = f.Write(append(append([]byte{'\n'}, b...), '\n'))
	closeErr := f.Close()
	if err != nil {
		return err
	}
	return closeErr
}
