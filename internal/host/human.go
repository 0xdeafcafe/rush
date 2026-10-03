package host

import (
	"bufio"
	"errors"
	"io/fs"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/jsonx"
)

// HumanMessage is a message a person typed and sent to a session, as
// opposed to one a script, another agent, a monitor or a background task
// delivered in the user's role. Only the sender knows which it is, so it
// is written down as it's sent: the message box does, and rush session
// send --human.
type HumanMessage struct {
	At   time.Time `json:"at"`
	Text string    `json:"text"`
	// UUID is the message's id in the agent's transcript, when it's known.
	UUID string `json:"uuid,omitempty"`
	// Guessed is set when the session has no record and the message was
	// picked out of its transcript instead.
	Guessed bool `json:"guessed,omitzero"`
}

// HumanPath is where a session's record of typed messages is kept: in its
// own folder, a JSON line each, so it outlives its host.
func HumanPath(id string) string { return filepath.Join(dir(id), "human.jsonl") }

// RecordHuman writes down that a person typed text and sent it to session
// id just now. A message of nothing but images is recorded with no text.
// The session must be there already: no folder is made for one that isn't.
func RecordHuman(id, text string) error { return RecordHumanAt(id, text, time.Now()) }

// RecordHumanAt is RecordHuman for a message sent at at: the moment it
// went, taken before the send, which the record is written after.
func RecordHumanAt(id, text string, at time.Time) error {
	b, err := jsonx.Marshal(HumanMessage{At: at.UTC().Truncate(time.Millisecond), Text: text})
	if err != nil {
		return err
	}
	if id == "" || strings.ContainsAny(id, "/\\") {
		return errors.New("no such session")
	}
	// One write of one line, appended: two senders never mix theirs.
	f, err := os.OpenFile(HumanPath(id), os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o600)
	if err != nil {
		return err
	}
	_, err = f.Write(append(b, '\n'))
	return errors.Join(err, f.Close())
}

// HumanMessages is what a person typed to session id, oldest first, and
// whether the session keeps a record at all: one from before the record
// has none, and its reader falls back to the transcript.
func HumanMessages(id string) (msgs []HumanMessage, recorded bool, err error) {
	f, err := os.Open(HumanPath(id))
	if errors.Is(err, fs.ErrNotExist) {
		return nil, false, nil
	}
	if err != nil {
		return nil, false, err
	}
	defer f.Close()
	sc := bufio.NewScanner(f)
	sc.Buffer(make([]byte, 0, 64<<10), 64<<20)
	for sc.Scan() {
		line := strings.TrimSpace(sc.Text())
		if line == "" {
			continue
		}
		var m HumanMessage
		if jsonx.Unmarshal([]byte(line), &m) != nil {
			continue // a line cut short by a crash: the rest still read
		}
		msgs = append(msgs, m)
	}
	return msgs, true, sc.Err()
}
