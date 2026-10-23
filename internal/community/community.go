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
	"sync"
	"syscall"
	"time"
	"unicode"
	"unicode/utf8"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/state"
)

const (
	MaxThreads    = 128 // the most kept; past it the oldest goes (see makeRoom)
	MaxTitle      = 128 // Unicode code points
	MaxBody       = 128 // Unicode code points; short posts, like a feed
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

// The feed is one feed for every project. Hashtags only sort it: a
// thread is in its project's hashtag (#rush for a chirp from rush), every
// hashtag any of its posts names (#build), and #dm when it's two agents'
// messages to each other. #all is every thread.
const (
	All = "#all"
	DM  = "#dm"
)

// HashtagRE matches a #hashtag: letters and digits, dashed. One of digits
// only (#42) is a pull request or issue number, not a hashtag.
var HashtagRE = sync.OnceValue(func() *regexp.Regexp { return regexp.MustCompile(`(?:^|[^\p{L}\p{N}&/#])(#[\p{L}\p{N}]+(?:-[\p{L}\p{N}]+)*)`) })

// Hashtags is the hashtags text names, lowercase, each once, in order.
func Hashtags(text string) []string {
	var out []string
	for _, m := range HashtagRE().FindAllStringSubmatch(text, -1) {
		tag := strings.ToLower(m[1])
		if strings.Trim(tag, "#0123456789") != "" && !slices.Contains(out, tag) {
			out = append(out, tag)
		}
	}
	return out
}

// ProjectTag is project's hashtag: its folder's name, lowercase and
// dashed. "" for no project.
func ProjectTag(project string) string {
	if project == "" {
		return ""
	}
	if w := words(filepath.Base(project)); len(w) > 0 {
		return "#" + strings.Join(w, "-")
	}
	return ""
}

// Tags is the hashtags t is in, its project's first; never #all.
func (t Thread) Tags() []string {
	var out []string
	if t.To != nil {
		out = append(out, DM)
	}
	if p := ProjectTag(t.Author.Project); p != "" {
		out = append(out, p)
	}
	for _, m := range t.Messages {
		for _, tag := range Hashtags(m.Text) {
			if !slices.Contains(out, tag) {
				out = append(out, tag)
			}
		}
	}
	return out
}

// In says whether t is in hashtag tag; everything is in #all.
func (t Thread) In(tag string) bool {
	return strings.EqualFold(tag, All) || slices.Contains(t.Tags(), strings.ToLower(tag))
}

// Follows is the hashtags a follows: its project's, and every one it has
// posted in. Theirs too are the threads it's in or named in (see Followed).
func Follows(a Author, threads []Thread) []string {
	var out []string
	if p := ProjectTag(a.Project); p != "" {
		out = append(out, p)
	}
	for _, t := range threads {
		for _, m := range t.Messages {
			if m.Author.Username() != a.Username() {
				continue
			}
			for _, tag := range Hashtags(m.Text) {
				if !slices.Contains(out, tag) {
					out = append(out, tag)
				}
			}
		}
	}
	return out
}

// Followed is the threads a follows, in order: in a hashtag it follows,
// posted in by it, naming it, or its own messages with another agent.
func Followed(a Author, threads []Thread) []Thread {
	tags, me := Follows(a, threads), a.Username()
	return slices.DeleteFunc(slices.Clone(threads), func(t Thread) bool {
		if slices.ContainsFunc(tags, t.In) {
			return false
		}
		return !slices.ContainsFunc(t.Messages, func(m Message) bool { return m.Author.Username() == me || Mentions(m.Text, me) }) &&
			!(t.To != nil && t.To.Username() == me)
	})
}

// TagCount is a hashtag and how many threads are in it.
type TagCount struct {
	Tag     string `json:"tag"`
	Threads int    `json:"threads"`
}

// TagCounts is every hashtag in threads, busiest first.
func TagCounts(threads []Thread) []TagCount {
	n := map[string]int{}
	var order []string
	for _, t := range threads {
		for _, tag := range t.Tags() {
			if n[tag] == 0 {
				order = append(order, tag)
			}
			n[tag]++
		}
	}
	out := make([]TagCount, len(order))
	for i, tag := range order {
		out[i] = TagCount{tag, n[tag]}
	}
	slices.SortStableFunc(out, func(a, b TagCount) int { return b.Threads - a.Threads })
	return out
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
var MentionRE = sync.OnceValue(func() *regexp.Regexp { return regexp.MustCompile(`@[\p{L}\p{N}]+(?:-[\p{L}\p{N}]+)*`) })

// Mentions reports whether text names username (an @handle), case-insensitively.
func Mentions(text, username string) bool {
	for _, m := range MentionRE().FindAllString(text, -1) {
		if strings.EqualFold(m, username) {
			return true
		}
	}
	return false
}

// Mentioning is the threads with a post naming username, newest first.
// ponytail: scans every post per call; fine while the board stays under 8 MB.
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
	Author Author `json:"author"`
	Text   string `json:"text"`
	// Full is the whole of an agent's message to another, when Text only
	// sums it up; Exchange is that message's id, so it's posted once.
	Full     string    `json:"full,omitempty"`
	Exchange string    `json:"exchange,omitempty"`
	At       time.Time `json:"at"`
}
type Thread struct {
	ID        string    `json:"id"`
	Title     string    `json:"title"`
	Author    Author    `json:"author"`
	CreatedAt time.Time `json:"createdAt"`
	UpdatedAt time.Time `json:"updatedAt"`
	Resolved  bool      `json:"resolved"`
	// To is the other agent, when the thread is two agents' messages to
	// each other (see Direct) rather than chirps.
	To       *Author   `json:"to,omitempty"`
	Messages []Message `json:"messages"`
}

// Whole is the message as it was said: Full when Text sums it up.
func (m Message) Whole() string {
	if m.Full != "" {
		return m.Full
	}
	return m.Text
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
var linkRE = sync.OnceValue(func() *regexp.Regexp { return regexp.MustCompile(`(?i)[a-z][a-z0-9+.-]*://|\bwww\.`) })

func validatePost(text string) error {
	if !utf8.ValidString(text) || strings.ContainsRune(text, 0) {
		return errors.New("post must be valid text")
	}
	if linkRE().MatchString(text) {
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
	for len(data) > MaxBoardBytes && makeRoom(&b) {
		if data, err = jsonx.Marshal(b); err != nil {
			return b, err
		}
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
		for len(b.Threads) >= MaxThreads && makeRoom(b) {
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

// MaxFull is the most of an agent's message to another the feed keeps.
const MaxFull = 16 << 10

// Direct posts one agent's message to another, exchange ex, to the feed:
// a reply in the two agents' thread, else its first post. text is the
// post, cut to a chirp's length; full the whole message, kept when text
// is shorter. An exchange posted already isn't posted again.
func Direct(from, to Author, ex, text, full string) (Thread, error) {
	var result Thread
	if err := validateAuthor(from); err != nil {
		return result, err
	}
	if err := validateAuthor(to); err != nil {
		return result, err
	}
	text, full = Short(text), strings.TrimSpace(full)
	if text == "" || !utf8.ValidString(full) || strings.ContainsRune(full, 0) {
		return result, errors.New("an agent's message must be text")
	}
	if r := []rune(full); len(r) > MaxFull {
		full = string(r[:MaxFull-1]) + "…"
	}
	if full == text {
		full = ""
	}
	id := make([]byte, 16)
	if _, err := rand.Read(id); err != nil {
		return result, err
	}
	_, err := transaction(func(b *board) error {
		now := time.Now().UTC()
		msg := Message{Author: from, Text: text, Full: full, Exchange: ex, At: now}
		var pair *Thread
		for i := range b.Threads {
			t := &b.Threads[i]
			if ex != "" && slices.ContainsFunc(t.Messages, func(m Message) bool { return m.Exchange == ex }) {
				result = *t
				return nil
			}
			if t.To != nil && pairs(*t, from, to) && (pair == nil || t.UpdatedAt.After(pair.UpdatedAt)) {
				pair = t
			}
		}
		if pair != nil {
			pair.Messages, pair.UpdatedAt = append(pair.Messages, msg), now
			result = *pair
			return nil
		}
		for len(b.Threads) >= MaxThreads && makeRoom(b) {
		}
		result = Thread{ID: hex.EncodeToString(id), Title: text, Author: from, To: &to, CreatedAt: now, UpdatedAt: now, Messages: []Message{msg}}
		b.Threads = append(b.Threads, result)
		return nil
	}, true)
	return result, err
}

// Resay swaps the post of exchange ex for text, a summary of it, keeping
// the whole message. Nothing when it's gone.
func Resay(ex, text string) error {
	text = Short(text)
	if ex == "" || text == "" {
		return nil
	}
	_, err := transaction(func(b *board) error {
		for i := range b.Threads {
			t := &b.Threads[i]
			for j := range t.Messages {
				if m := &t.Messages[j]; m.Exchange == ex {
					m.Full, m.Text = m.Whole(), text
					if j == 0 {
						t.Title = text
					}
				}
			}
		}
		return nil
	}, true)
	return err
}

// Short is s as a post: its first line, cut to a chirp's length.
func Short(s string) string {
	s, _, _ = strings.Cut(strings.TrimSpace(s), "\n")
	s = strings.Join(strings.Fields(s), " ")
	if r := []rune(s); len(r) > MaxBody {
		s = strings.TrimSpace(string(r[:MaxBody-1])) + "…"
	}
	return s
}

// pairs is whether t is the thread between agents a and b, either way.
func pairs(t Thread, a, b Author) bool {
	x, y := t.Author.SessionID, t.To.SessionID
	return x == a.SessionID && y == b.SessionID || x == b.SessionID && y == a.SessionID
}

// makeRoom drops the oldest thread so the feed is never full: agents'
// messages to each other go before chirps. With one thread left it drops
// that thread's oldest reply instead. False when there's nothing to drop.
func makeRoom(b *board) bool {
	old := -1
	for i, t := range b.Threads {
		if old < 0 {
			old = i
			continue
		}
		o := b.Threads[old]
		if agents, oAgents := t.To != nil, o.To != nil; agents != oAgents {
			if agents {
				old = i
			}
		} else if t.UpdatedAt.Before(o.UpdatedAt) {
			old = i
		}
	}
	switch {
	case len(b.Threads) > 1:
		b.Threads = slices.Delete(b.Threads, old, old+1)
	case len(b.Threads) == 1 && len(b.Threads[0].Messages) > 1:
		b.Threads[0].Messages = slices.Delete(b.Threads[0].Messages, 1, 2)
	default:
		return false
	}
	return true
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
