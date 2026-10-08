package convo

import (
	"bufio"
	"bytes"
	"io"
	"os"

	"github.com/0xdeafcafe/photon/jsonx"
)

// TurnStart is where a turn begins in a transcript: the byte offset of the
// line that opened it, numbered as the conversation view numbers turns.
type TurnStart struct {
	N      int
	Prompt string
	Offset int64
	// UUID is the message's id in the transcript, which Claude Code's
	// file checkpoints are kept under.
	UUID string
	// From is set when you didn't send it (see Turn.From).
	From string
}

// TurnStarts reads a transcript and says where each turn starts, so a copy
// cut there holds the conversation as it was before that turn.
func TurnStarts(path string) ([]TurnStart, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	t := NewTail(path)
	r := bufio.NewReaderSize(f, 256<<10)
	var out []TurnStart
	var off int64
	for {
		line, err := r.ReadBytes('\n')
		if len(line) > 0 && (err == nil || err == io.EOF) {
			n := len(t.Sess.Turns)
			t.apply(bytes.TrimSpace(line))
			if len(t.Sess.Turns) > n {
				tn := t.Sess.Turns[len(t.Sess.Turns)-1]
				var id struct {
					UUID string `json:"uuid"`
				}
				_ = jsonx.Unmarshal(line, &id)
				out = append(out, TurnStart{N: tn.N, Prompt: tn.Prompt, Offset: off, UUID: id.UUID, From: tn.From})
			}
			off += int64(len(line))
		}
		if err != nil {
			if err == io.EOF {
				return out, nil
			}
			return out, err
		}
	}
}
