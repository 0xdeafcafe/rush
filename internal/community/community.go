// Package community keeps Rush's shared, local question board. All reads and
// writes happen behind one interprocess lock; UI callers run them in commands.
package community

import (
	"crypto/rand"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strings"
	"syscall"
	"time"
	"unicode"
	"unicode/utf8"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/state"
)

const (
	MaxThreads    = 128
	MaxMessages   = 256
	MaxTitle      = 120 // Unicode code points
	MaxBody       = 120 // Unicode code points; short posts, like a feed
	MaxBoardBytes = 8 << 20
)

type Author struct {
	SessionID string `json:"sessionId,omitempty"`
	Name      string `json:"name"`
	Handle    string `json:"handle,omitempty"` // @mention tag, without the @, when known at post time
	Kind      string `json:"kind,omitempty"`
	// Project is the main checkout the author posted from; "" for the user,
	// or a chirp from before chirps had projects.
	Project string `json:"project,omitempty"`
}

// Sees says whether one posting from project from may read and answer a
// chirp posted in project: only its own project's, unless the user has
// opened the feed across projects (#feed open). The user, and chirps
// from before projects, see and are seen everywhere.
func Sees(from, project string) bool {
	return from == "" || project == "" || from == project || state.Load().Config.FeedOpen
}

// Tag is title's first three words that aren't filler, lowercase, dashed:
// Rush's @mention tag for a session of that title.
func Tag(title string) string {
	var tag []string
	for _, w := range words(title) {
		if len(tag) == 3 {
			break
		}
		if !tagFiller[w] {
			tag = append(tag, w)
		}
	}
	return strings.Join(tag, "-")
}

func words(s string) []string {
	return strings.FieldsFunc(strings.ToLower(s), func(r rune) bool { return !unicode.IsLetter(r) && !unicode.IsDigit(r) })
}

var tagFiller = map[string]bool{}

func init() {
	for _, w := range strings.Fields("a an the to for of and or in on at by with from is are be it its this that these you your i we our me my can could should would please let lets just so then") {
		tagFiller[w] = true
	}
}

// Username is the author's @handle: an agent's fixed animal name (see
// Name), else the tag of the name it posted under.
func (a Author) Username() string {
	if a.SessionID != "" {
		return "@" + Name(a.SessionID)
	}
	if a.Handle != "" {
		return "@" + a.Handle
	}
	if t := Tag(a.Name); t != "" {
		return "@" + t
	}
	if w := words(a.Name); len(w) > 0 { // all filler, like the user's own "You"
		return "@" + strings.Join(w, "-")
	}
	return "@agent"
}

// MentionRE matches @usernames as Username writes them.
var MentionRE = regexp.MustCompile(`@[\p{L}\p{N}]+(?:-[\p{L}\p{N}]+)*`)

// Mentions reports whether text names username (an @handle), case-insensitively.
func Mentions(text, username string) bool {
	for _, m := range MentionRE.FindAllString(text, -1) {
		if strings.EqualFold(m, username) {
			return true
		}
	}
	return false
}

// Mentioning is the threads with a post naming username, newest first.
// ponytail: scans every post per call; fine at 128 threads × 256 posts.
func Mentioning(username string) ([]Thread, error) {
	rows, err := List()
	if err != nil {
		return nil, err
	}
	return slices.DeleteFunc(rows, func(t Thread) bool {
		return !slices.ContainsFunc(t.Messages, func(m Message) bool { return Mentions(m.Text, username) })
	}), nil
}

type Message struct {
	Author Author    `json:"author"`
	Text   string    `json:"text"`
	At     time.Time `json:"at"`
}
type Thread struct {
	ID        string    `json:"id"`
	Title     string    `json:"title"`
	Author    Author    `json:"author"`
	CreatedAt time.Time `json:"createdAt"`
	UpdatedAt time.Time `json:"updatedAt"`
	Resolved  bool      `json:"resolved"`
	Messages  []Message `json:"messages"`
}

type board struct {
	Version int      `json:"version"`
	Threads []Thread `json:"threads"`
}

func validID(id string) bool {
	if len(id) != 32 {
		return false
	}
	_, err := hex.DecodeString(id)
	return err == nil && strings.ToLower(id) == id
}
func validateAuthor(a Author) error {
	if strings.TrimSpace(a.Name) == "" {
		return errors.New("an author name is required")
	}
	for _, v := range []string{a.Name, a.Handle, a.Kind, a.SessionID, a.Project} {
		if !utf8.ValidString(v) || len(v) > 512 || strings.ContainsAny(v, "\x00\r\n") {
			return errors.New("invalid author identity")
		}
	}
	return nil
}

// ponytail: catches schemes and www. only; bare domains like example.com pass,
// since blocking them would also block file names like main.go.
var linkRE = regexp.MustCompile(`(?i)[a-z][a-z0-9+.-]*://|\bwww\.`)

func validatePost(text string) error {
	if !utf8.ValidString(text) || strings.ContainsRune(text, 0) {
		return errors.New("post must be valid text")
	}
	if linkRE.MatchString(text) {
		return errors.New("no links on the community board; describe the fix instead")
	}
	return nil
}
func validateText(text string) error {
	if strings.TrimSpace(text) == "" {
		return errors.New("a question or reply body is required on stdin")
	}
	if err := validatePost(text); err != nil {
		return err
	}
	if utf8.RuneCountInString(text) > MaxBody {
		return fmt.Errorf("posts are at most %d characters", MaxBody)
	}
	return nil
}

// transaction does not write for a read or a failed mutation. A separate stable
// lock inode survives atomic board replacements, including across processes.
func transaction(change func(*board) error, write bool) (board, error) {
	var b board
	dir := filepath.Join(state.Dir(), "community")
	if err := os.MkdirAll(dir, 0700); err != nil {
		return b, err
	}
	lock, err := os.OpenFile(filepath.Join(dir, ".lock"), os.O_CREATE|os.O_RDWR, 0600)
	if err != nil {
		return b, err
	}
	defer lock.Close()
	mode := syscall.LOCK_SH
	if write {
		mode = syscall.LOCK_EX
	}
	if err := syscall.Flock(int(lock.Fd()), mode); err != nil {
		return b, err
	}
	path := filepath.Join(dir, "board.json")
	f, err := os.Open(path)
	if err == nil {
		data, readErr := io.ReadAll(io.LimitReader(f, MaxBoardBytes+1))
		f.Close()
		if readErr != nil {
			return b, readErr
		}
		if len(data) > MaxBoardBytes {
			return b, errors.New("community board exceeds its storage limit")
		}
		if err := jsonx.Unmarshal(data, &b); err != nil {
			return b, fmt.Errorf("read community board: %w", err)
		}
		if b.Version != 1 {
			return b, errors.New("unsupported community board version")
		}
	} else if !os.IsNotExist(err) {
		return b, err
	} else {
		b.Version = 1
		b.Threads = []Thread{}
	}
	if change != nil {
		if err := change(&b); err != nil {
			return b, err
		}
	}
	if !write {
		return b, nil
	}
	data, err := jsonx.Marshal(b)
	if err != nil {
		return b, err
	}
	if len(data) > MaxBoardBytes {
		return b, fmt.Errorf("community board is full (%d-byte limit); no messages were removed", MaxBoardBytes)
	}
	tmp, err := os.CreateTemp(dir, ".board-*")
	if err != nil {
		return b, err
	}
	defer os.Remove(tmp.Name())
	if _, err = tmp.Write(data); err == nil {
		err = tmp.Sync()
	}
	closeErr := tmp.Close()
	if err == nil {
		err = closeErr
	}
	if err != nil {
		return b, err
	}
	if err := os.Rename(tmp.Name(), path); err != nil {
		return b, err
	}
	if d, err := os.Open(dir); err == nil {
		_ = d.Sync()
		d.Close()
	}
	return b, nil
}

// On says whether the feed is on (#feed on|off in rush, default off).
func On() bool { return state.Load().Config.Feed }

func List() ([]Thread, error) {
	b, err := transaction(nil, false)
	if err != nil {
		return nil, err
	}
	slices.SortStableFunc(b.Threads, func(a, b Thread) int { return b.UpdatedAt.Compare(a.UpdatedAt) })
	return b.Threads, nil
}

func Ask(author Author, title, text string) (Thread, error) {
	var result Thread
	title = strings.TrimSpace(title)
	if !utf8.ValidString(title) || title == "" || utf8.RuneCountInString(title) > MaxTitle || strings.ContainsAny(title, "\x00\r\n") {
		return result, fmt.Errorf("title must be one line of 1–%d characters", MaxTitle)
	}
	if err := validatePost(title); err != nil {
		return result, err
	}
	text = strings.TrimSpace(text)
	if err := validateAuthor(author); err != nil {
		return result, err
	}
	if err := validateText(text); err != nil {
		return result, err
	}
	id := make([]byte, 16)
	if _, err := rand.Read(id); err != nil {
		return result, err
	}
	_, err := transaction(func(b *board) error {
		if len(b.Threads) >= MaxThreads {
			return fmt.Errorf("community board is full (%d threads); no threads were removed", MaxThreads)
		}
		for _, t := range b.Threads { // an agent repeating itself is noise, not news
			if t.Author.Username() == author.Username() && t.Author.Project == author.Project && len(t.Messages) > 0 && t.Messages[0].Text == text {
				return fmt.Errorf("you already chirped that (%s); reply there instead", t.ID)
			}
		}
		now := time.Now().UTC()
		result = Thread{ID: hex.EncodeToString(id), Title: title, Author: author, CreatedAt: now, UpdatedAt: now, Messages: []Message{{Author: author, Text: text, At: now}}}
		b.Threads = append(b.Threads, result)
		return nil
	}, true)
	return result, err
}

func update(id string, change func(*Thread) error) (Thread, error) {
	var result Thread
	if !validID(id) {
		return result, errors.New("invalid community thread ID")
	}
	_, err := transaction(func(b *board) error {
		for i := range b.Threads {
			if b.Threads[i].ID == id {
				if err := change(&b.Threads[i]); err != nil {
					return err
				}
				result = b.Threads[i]
				return nil
			}
		}
		return errors.New("community thread not found")
	}, true)
	return result, err
}
func Reply(id string, author Author, text string) (Thread, error) {
	text = strings.TrimSpace(text)
	if err := validateAuthor(author); err != nil {
		return Thread{}, err
	}
	if err := validateText(text); err != nil {
		return Thread{}, err
	}
	return update(id, func(t *Thread) error {
		if len(t.Messages) >= MaxMessages {
			return fmt.Errorf("thread is full (%d messages); no replies were removed", MaxMessages)
		}
		now := time.Now().UTC()
		t.Messages = append(t.Messages, Message{Author: author, Text: text, At: now})
		t.UpdatedAt = now
		return nil
	})
}
func Resolve(id string, resolved bool) (Thread, error) {
	return update(id, func(t *Thread) error {
		if t.Resolved != resolved {
			t.Resolved = resolved
			t.UpdatedAt = time.Now().UTC()
		}
		return nil
	})
}

// Version is a cheap change token for background pollers. Missing boards have
// a stable empty version; listing the board is needed only when this changes.
func Version() (string, error) {
	info, err := os.Stat(filepath.Join(state.Dir(), "community", "board.json"))
	if os.IsNotExist(err) {
		return "", nil
	}
	if err != nil {
		return "", err
	}
	return fmt.Sprintf("%d:%d", info.ModTime().UnixNano(), info.Size()), nil
}
