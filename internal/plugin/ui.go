package plugin

import (
	"errors"
	"fmt"
	"regexp"
	"slices"
	"strings"
	"time"
	"unicode/utf8"
)

// A plugin can take part in rush's own screen, as far as its manifest's
// "ui" list says: hear what happens there, add to it, and have a say before
// a message goes. It never runs on rush's UI goroutine. The UI hands the
// broker events without waiting, draws what plugins added from a copy it
// already holds, and waits for an intercept only in the background, and
// only so long, before sending the message as it was.

// UI capabilities.
const (
	// UIEvents hears what happens in rush's screen: a Session opened or
	// left, a turn started or ended, a session stopped by an error and of
	// what kind, the API going away and coming back. What the agent list
	// shows of a session, never what was said.
	UIEvents = "events"
	// UIInput sees what you type in a message box (as it changes, and when
	// it's sent or cleared), may set it, and may put a note on its edge.
	// It is what a drafts feature needs, and it is everything you type:
	// approve it with care.
	UIInput = "input"
	// UIIntercept is asked before a message you send goes, and may change
	// it, hold it back with a reason shown, or ask you a question first. It
	// needs "input". It has InterceptBudget to answer; after that the
	// message goes as it was. Your answer to its question it has
	// AnswerWait for.
	UIIntercept = "intercept"
	// UIOverview adds sections to a Session's overview, and a short status
	// to its row in the list.
	UIOverview = "overview"
	// UINotify shows a short message at the bottom of rush's screen.
	UINotify = "notify"
	// UISend sends a message to a session as if you'd typed it and pressed
	// enter, or continues one an error stopped. It needs "events", and
	// reaches only sessions in the manifest's workspaces that ask before
	// acting, as "queue" does.
	UISend = "send"
)

var uiCaps = []string{UIEvents, UIInput, UIIntercept, UIOverview, UINotify, UISend}

// InterceptBudget is how long rush waits for all intercepts of one
// message together, and each plugin's share of it.
const (
	InterceptBudget    = 400 * time.Millisecond
	InterceptEach      = 250 * time.Millisecond
	InterceptStrikeOut = 3 // timeouts in a row before it's skipped until restarted
	// AnswerWait is how long a plugin has to act on the choice you made
	// in its question, as running a program it may take a while.
	AnswerWait = 2 * time.Minute
)

// Limits on what a plugin adds to the screen.
const (
	MaxCommands        = 32
	MaxSettings        = 32
	MaxSections        = 4   // per session, per plugin
	MaxSectionLines    = 24  //
	MaxLineLen         = 200 // characters
	MaxStatusLen       = 24
	MaxNoteLen         = 60 // on a message box's edge
	MaxNotifyLen       = 200
	MaxSessionsTracked = 2000
)

// CommandSpec is a command a plugin adds: to the # commands, the command
// bar and the keymap, as plugin:<name>.<command>.
type CommandSpec struct {
	Name        string `json:"name"`
	Description string `json:"description"`
	// Key is a key it suggests, taken only if it's free. You can change it
	// in Settings, Keys.
	Key string `json:"key,omitempty"`
}

// SettingSpec is a setting a plugin offers, shown under Settings, Plugins.
// rush keeps its value, in a file the plugin can't write, and hands it the
// values at initialize and when you change one.
type SettingSpec struct {
	Key         string   `json:"key"`
	Title       string   `json:"title"`
	Description string   `json:"description,omitempty"`
	Type        string   `json:"type"`              // bool, choice or text
	Choices     []string `json:"choices,omitempty"` // for choice
	Default     string   `json:"default,omitempty"` // "true"/"false" for bool
}

var cmdRE = regexp.MustCompile(`^[a-z][a-z0-9-]{0,30}$`)

// CanUI says whether it was given UI capability c.
func (m *Manifest) CanUI(c string) bool { return slices.Contains(m.UI, c) }

// validateUI checks the manifest's ui, commands and settings.
func (m *Manifest) validateUI() error {
	if (len(m.UI) > 0 || len(m.Commands) > 0 || len(m.Settings) > 0) && m.Proto() != ProtoRush {
		return errors.New("an MCP plugin cannot take part in rush's screen: it has no way to hear from it")
	}
	for _, c := range m.UI {
		if !slices.Contains(uiCaps, c) {
			return fmt.Errorf("ui capability %q: use one of %s", c, strings.Join(uiCaps, ", "))
		}
	}
	if m.CanUI(UIIntercept) && !m.CanUI(UIInput) {
		return errors.New(`"intercept" needs "input": it's shown the message it's asked about`)
	}
	if m.CanUI(UISend) {
		if !m.CanUI(UIEvents) {
			return errors.New(`"send" needs "events", to know which session it sends to`)
		}
		if len(m.Workspaces) == 0 {
			return errors.New(`"send" needs at least one workspace whose sessions it may send to`)
		}
	}
	if err := m.validateCommands(); err != nil {
		return err
	}
	return m.validateSettings()
}

func (m *Manifest) validateCommands() error {
	if len(m.Commands) > MaxCommands {
		return fmt.Errorf("over %d commands", MaxCommands)
	}
	seen := map[string]bool{}
	for _, c := range m.Commands {
		if !cmdRE.MatchString(c.Name) {
			return fmt.Errorf("command %q: use lowercase letters, digits and dashes", c.Name)
		}
		if seen[c.Name] {
			return fmt.Errorf("command %q twice", c.Name)
		}
		seen[c.Name] = true
		if c.Description == "" || utf8.RuneCountInString(c.Description) > 200 {
			return fmt.Errorf("command %q needs a description of at most 200 characters", c.Name)
		}
		if c.Key != "" && (strings.ContainsAny(c.Key, "\n\t") || len(c.Key) > 40) {
			return fmt.Errorf("command %q: key %q is not a key", c.Name, c.Key)
		}
	}
	return nil
}

func (m *Manifest) validateSettings() error {
	if len(m.Settings) > MaxSettings {
		return fmt.Errorf("over %d settings", MaxSettings)
	}
	seen := map[string]bool{}
	for _, s := range m.Settings {
		if !cmdRE.MatchString(s.Key) {
			return fmt.Errorf("setting %q: use lowercase letters, digits and dashes", s.Key)
		}
		if seen[s.Key] {
			return fmt.Errorf("setting %q twice", s.Key)
		}
		seen[s.Key] = true
		if err := s.validate(); err != nil {
			return err
		}
	}
	return nil
}

func (s SettingSpec) validate() error {
	if s.Title == "" || utf8.RuneCountInString(s.Title) > 60 {
		return fmt.Errorf("setting %q needs a title of at most 60 characters", s.Key)
	}
	switch s.Type {
	case "bool":
		if s.Default != "" && s.Default != "true" && s.Default != "false" {
			return fmt.Errorf("setting %q: a bool's default is true or false", s.Key)
		}
	case "choice":
		if len(s.Choices) < 2 || len(s.Choices) > 16 {
			return fmt.Errorf("setting %q: a choice needs 2 to 16 choices", s.Key)
		}
		if s.Default != "" && !slices.Contains(s.Choices, s.Default) {
			return fmt.Errorf("setting %q: default %q is not one of its choices", s.Key, s.Default)
		}
	case "text":
		if len(s.Default) > 1000 {
			return fmt.Errorf("setting %q: default is too long", s.Key)
		}
	default:
		return fmt.Errorf("setting %q: type %q, use bool, choice or text", s.Key, s.Type)
	}
	return nil
}

// UISession is what a UI event says about a session: what the list shows.
type UISession struct {
	ID        string `json:"id"`                  // rush's key for it
	SessionID string `json:"sessionId,omitempty"` // Claude Code's (or the agent's) id
	Name      string `json:"name,omitempty"`
	Agent     string `json:"agent,omitempty"` // claude, codex, …
	Cwd       string `json:"cwd,omitempty"`
	Repo      string `json:"repo,omitempty"`
	Branch    string `json:"branch,omitempty"`
	State     string `json:"state,omitempty"`
	Hosted    bool   `json:"hosted,omitzero"` // a rush-mode session
}

// UI event kinds.
const (
	EvSessionSeen    = "session.seen"    // one rush shows: each recent one when a plugin connects, and each new one
	EvSessionOpened  = "session.opened"  // its Session came into view
	EvSessionLeft    = "session.left"    // its Session went out of view
	EvTurnStarted    = "turn.started"    //
	EvTurnEnded      = "turn.ended"      //
	EvSessionStopped = "session.stopped" // Error says why, when an error did it
	EvNetworkDown    = "network.down"    // the API stopped answering
	EvNetworkUp      = "network.up"      // and answers again
	EvInputChanged   = "input.changed"   // "input" only; Text is the box
	EvInputSent      = "input.sent"      // "input" only
	EvInputCleared   = "input.cleared"   // "input" only
	EvCommand        = "command"         // not sent: commands come as ui.command requests
)

// UIEvent is one thing that happened in rush's screen.
type UIEvent struct {
	Kind    string     `json:"kind"`
	UI      string     `json:"ui"` // which rush window
	At      time.Time  `json:"at"`
	Session *UISession `json:"session,omitempty"`
	// Error is why a session stopped: limit, auth, offline, retryable,
	// too-long or other, and what it said.
	Error *UIError `json:"error,omitempty"`
	// Text is the message box, for input events, to plugins with "input".
	Text string `json:"text,omitempty"`
	// Box is the whole box, for input events: Text with where the cursor
	// is and what its chips stand for.
	Box *Box `json:"box,omitempty"`
}

// Box is a message box as a plugin with "input" sees it, and may set it.
// Text is as the box shows it: a paste as [#N L lines: start…end],
// an image as [Image #N]. Setting a Box puts all of it back as it was.
type Box struct {
	Text   string         `json:"text"`
	Cursor int            `json:"cursor"`           // in characters from the start
	Pastes map[int]string `json:"pastes,omitempty"` // each paste chip's text, by N
	// Images are each [Image #N]'s file. The Prompt, whose images are
	// attachments rather than in its text, numbers them 1, 2, 3.
	Images map[int]string `json:"images,omitempty"`
}

// Size is about how many bytes the box holds, to check against limits.
func (b *Box) Size() int {
	if b == nil {
		return 0
	}
	n := len(b.Text)
	for _, p := range b.Pastes {
		n += len(p)
	}
	for _, p := range b.Images {
		n += len(p)
	}
	return n
}

// UIError is why a session stopped.
type UIError struct {
	Kind    string `json:"kind"`
	Message string `json:"message,omitempty"`
	// Retrying is rush telling the session to continue itself, as it does
	// after an offline or retryable error; a plugin needn't too.
	Retrying bool `json:"retrying,omitempty"`
}

// InputEvent is whether the event carries what you typed.
func (e UIEvent) InputEvent() bool { return strings.HasPrefix(e.Kind, "input.") }

// For is the event as plugin m may see it, or false if it may not.
func (e UIEvent) For(m *Manifest) (UIEvent, bool) {
	if e.InputEvent() {
		return e, m.CanUI(UIInput)
	}
	if !m.CanUI(UIEvents) {
		return UIEvent{}, false
	}
	e.Text, e.Box = "", nil
	return e, true
}

// Line is a line a plugin draws, with a tone rush colours it by.
type Line struct {
	Text string `json:"text"`
	Tone string `json:"tone,omitempty"` // "", dim, good, warn, bad, accent
	URL  string `json:"url,omitempty"`  // opened with enter, if http(s)
}

var tones = []string{"", "dim", "good", "warn", "bad", "accent"}

// Section is a titled block a plugin adds to a Session's overview.
type Section struct {
	ID    string `json:"id"`
	Title string `json:"title"`
	Lines []Line `json:"lines"`
}

// Status is a short word or two a plugin puts on a session's row.
type Status struct {
	Text string `json:"text"`
	Tone string `json:"tone,omitempty"`
}

// CleanLine makes a line safe to draw: no escapes or control characters,
// no longer than MaxLineLen, a known tone, and a URL only if it's http(s).
func CleanLine(l Line) Line {
	l.Text = clip(printable(l.Text), MaxLineLen)
	if !slices.Contains(tones, l.Tone) {
		l.Tone = ""
	}
	if !strings.HasPrefix(l.URL, "https://") && !strings.HasPrefix(l.URL, "http://") || len(l.URL) > 2000 || strings.ContainsAny(l.URL, " \n\t\x1b") {
		l.URL = ""
	}
	return l
}

// CleanSection makes a section safe to draw, or says why it can't be.
func CleanSection(s Section) (Section, error) {
	if !cmdRE.MatchString(s.ID) {
		return s, fmt.Errorf("section id %q: use lowercase letters, digits and dashes", s.ID)
	}
	s.Title = clip(printable(s.Title), 60)
	if len(s.Lines) > MaxSectionLines {
		s.Lines = s.Lines[:MaxSectionLines]
	}
	for i := range s.Lines {
		s.Lines[i] = CleanLine(s.Lines[i])
	}
	return s, nil
}

// CleanStatus makes a status safe to draw.
func CleanStatus(s Status) Status {
	s.Text = clip(printable(s.Text), MaxStatusLen)
	if !slices.Contains(tones, s.Tone) {
		s.Tone = ""
	}
	return s
}

// CleanNote makes a note on a message box's edge safe to draw.
func CleanNote(s Status) Status {
	s.Text = clip(printable(s.Text), MaxNoteLen)
	if !slices.Contains(tones, s.Tone) {
		s.Tone = ""
	}
	return s
}

// CleanNotice makes a notice safe to draw.
func CleanNotice(s string) string { return clip(printable(s), MaxNotifyLen) }

// printable drops what a terminal would act on rather than show: escapes,
// control characters, and the bidi overrides that make text read other
// than it is.
func printable(s string) string {
	return strings.Map(func(r rune) rune {
		switch {
		case r == '\t':
			return ' '
		case r < 0x20, r == 0x7f, r >= 0x80 && r < 0xa0:
			return -1
		case r >= 0x202a && r <= 0x202e, r >= 0x2066 && r <= 0x2069, r == 0x200e, r == 0x200f:
			return -1
		case r == utf8.RuneError:
			return -1
		}
		return r
	}, s)
}

func clip(s string, n int) string {
	if utf8.RuneCountInString(s) <= n {
		return s
	}
	r := []rune(s)
	return string(r[:n-1]) + "…"
}

// Contributions a UI draws from, as the broker holds them.
type UIState struct {
	Plugins []UIPlugin `json:"plugins"`
	// Sections are each session's overview sections, by rush session id,
	// each plugin's in the order it gave them.
	Sections map[string][]UISection `json:"sections,omitempty"`
	// Statuses are each session's row statuses, by rush session id.
	Statuses map[string][]UIStatus `json:"statuses,omitempty"`
	// Notes are what plugins put on the edge of a message box: a
	// session's, by rush session id, or the Prompt's, by "".
	Notes map[string][]UIStatus `json:"notes,omitempty"`
}

// UIPlugin is what the UI needs of a plugin taking part in its screen.
type UIPlugin struct {
	Name     string            `json:"name"`
	Commands []CommandSpec     `json:"commands,omitempty"`
	Settings []SettingSpec     `json:"settings,omitempty"`
	Values   map[string]string `json:"values,omitempty"`
	UI       []string          `json:"ui,omitempty"`
	// Skipped is why its intercepts are skipped for now, if they are.
	Skipped string `json:"skipped,omitempty"`
}

// UISection is a plugin's section.
type UISection struct {
	Plugin string `json:"plugin"`
	Section
}

// UIStatus is a plugin's status.
type UIStatus struct {
	Plugin string `json:"plugin"`
	Status
}

// UIDo is something a plugin asks one rush window (or, with no UI, every
// one) to do now.
type UIDo struct {
	Plugin  string `json:"plugin"`
	UI      string `json:"ui,omitempty"`
	Kind    string `json:"kind"` // notify, input.set, send, pick
	Session string `json:"session,omitempty"`
	Text    string `json:"text,omitempty"`
	Tone    string `json:"tone,omitempty"`
	// Box, for input.set, is the whole box to put back; If, when set, is
	// the text the box must hold for it to be set at all, so what's typed
	// meanwhile is never lost.
	Box *Box    `json:"box,omitempty"`
	If  *string `json:"if,omitempty"`
	// Pick, for pick, is the list to show.
	Pick *Pick `json:"pick,omitempty"`
}

// Intercept is what a plugin is asked before a message goes.
type Intercept struct {
	Hook string `json:"hook"` // before-send
	UI   string `json:"ui"`
	// Box is whose message box it's in: a session's id, or "" for the
	// Prompt. Session is the session it goes to, absent for one it starts.
	Box     string     `json:"box"`
	Session *UISession `json:"session,omitempty"`
	Text    string     `json:"text"`
	// Asks are the kinds of ask the window can show beside one with
	// choices: AskInput. Absent from a window or a broker that knows only
	// choices, so a plugin sends an ask with an input only when it's here.
	Asks []string `json:"asks,omitempty"`
}

// AskInput, in an intercept's Asks, is an ask with a line of text to type.
const AskInput = "input"

// InterceptResult is a plugin's answer: let it go as it is, go changed,
// be held back, or ask you first.
type InterceptResult struct {
	Action string `json:"action"` // allow, rewrite, block, ask
	// Text, for rewrite, is the whole message as it should go.
	Text string `json:"text,omitempty"`
	// Replace and Append, for rewrite or ask, change the message in
	// place, so a window keeps the box's pastes as chips: each Old becomes
	// New wherever it is, then Append goes at the end, as it is.
	Replace []Replacement `json:"replace,omitempty"`
	Append  string        `json:"append,omitempty"`
	Reason  string        `json:"reason,omitempty"`
	// An ask: the question, a line under it, and the keys that answer.
	// The window shows it in place of sending, and hands the key chosen
	// back to the plugin that asked (ui.intercept.answer), with ID.
	ID       string      `json:"id,omitempty"`
	Question string      `json:"question,omitempty"`
	Detail   string      `json:"detail,omitempty"`
	Choices  []AskChoice `json:"choices,omitempty"`
	// Input makes the ask a line of text to type rather than a key to
	// choose: enter answers with what was typed.
	Input *AskLine `json:"input,omitempty"`
	// Plugin is who changed, held or asked about it, filled in by the
	// broker.
	Plugin string `json:"plugin,omitempty"`
}

// Replacement is text a rewrite changes wherever it is in a message.
type Replacement struct {
	Old string `json:"old"`
	New string `json:"new"`
}

// AskChoice is a key that answers a plugin's question. Enter, on one,
// makes enter choose it too, and Esc esc; ctrl+c, and esc when no choice
// takes it, cancel the send and leave the box as it is.
type AskChoice struct {
	Key   string `json:"key"`
	Label string `json:"label"`
	Enter bool   `json:"enter,omitempty"`
	Esc   bool   `json:"esc,omitempty"`
}

// AskLine is the line of text an ask has you type: Value is what it
// starts with, to edit in place, Error why what was typed last wasn't
// taken, shown under it, and Enter what enter does with it, in a word or
// two.
type AskLine struct {
	Value string `json:"value,omitempty"`
	Error string `json:"error,omitempty"`
	Enter string `json:"enter,omitempty"`
}

// KeyEnter is the Key of an answer to an ask with an input: enter, with
// what was typed in Value.
const KeyEnter = "enter"

// InterceptAnswer is the choice made in a plugin's question: the window
// sends it to the broker, which hands Key and ID to Plugin with the
// message as it stood, then asks the plugins after it. An ask with an
// input is answered with KeyEnter and Value, what was typed.
type InterceptAnswer struct {
	Intercept
	Plugin string `json:"plugin"`
	ID     string `json:"id,omitempty"`
	Key    string `json:"key"`
	Value  string `json:"value,omitempty"`
}

// Limits on an ask.
const (
	MaxChoices     = 6
	MaxQuestionLen = 120
	MaxDetailLen   = 400
	MaxChoiceLen   = 40
	MaxAskIDLen    = 128
	MaxInputLen    = 200
)

var askKeyRE = regexp.MustCompile(`^[a-z0-9]$`)

// CleanAsk checks an ask and cleans its words for the screen: one to
// MaxChoices choices, each a lowercase letter or a digit of its own, at
// most one on enter and one on esc. With an input, letters and digits are
// typed and enter answers, so its only choice, if it has one, is on esc.
func CleanAsk(r *InterceptResult) error {
	if strings.TrimSpace(r.Question) == "" {
		return errors.New("an ask needs a question")
	}
	if in := r.Input; in != nil {
		if len(r.Choices) > 1 || len(r.Choices) == 1 && (!r.Choices[0].Esc || r.Choices[0].Enter) {
			return errors.New("an ask with an input has at most one choice, on esc")
		}
		in.Value = clip(oneLine(in.Value), MaxInputLen)
		in.Error = clip(printable(in.Error), MaxInputLen)
		in.Enter = clip(printable(in.Enter), MaxChoiceLen)
	} else if len(r.Choices) == 0 || len(r.Choices) > MaxChoices {
		return fmt.Errorf("an ask has 1 to %d choices", MaxChoices)
	}
	if len(r.ID) > MaxAskIDLen {
		return fmt.Errorf("an ask's id is at most %d bytes", MaxAskIDLen)
	}
	r.Question, r.Detail = clip(printable(r.Question), MaxQuestionLen), clip(printable(r.Detail), MaxDetailLen)
	seen, enter, esc := map[string]bool{}, 0, 0
	for i := range r.Choices {
		c := &r.Choices[i]
		if !askKeyRE.MatchString(c.Key) || seen[c.Key] {
			return fmt.Errorf("choice key %q: a lowercase letter or a digit, each once", c.Key)
		}
		seen[c.Key] = true
		c.Label = clip(printable(c.Label), MaxChoiceLen)
		if c.Enter {
			enter++
		}
		if c.Esc {
			esc++
		}
	}
	if enter > 1 || esc > 1 {
		return errors.New("at most one choice on enter and one on esc")
	}
	return nil
}

// CleanInput is what was typed in an ask's input, as it's handed to the
// plugin: one line, at most MaxInputLen.
func CleanInput(s string) string { return clip(oneLine(s), MaxInputLen) }

// oneLine is s cleaned for the screen with its line breaks as spaces.
func oneLine(s string) string {
	return printable(strings.NewReplacer("\r\n", " ", "\n", " ", "\r", " ").Replace(s))
}

// Apply is text with r's change: its whole Text, or its replacements then
// what it appends.
func (r InterceptResult) Apply(text string) string {
	if r.Text != "" {
		return r.Text
	}
	for _, p := range r.Replace {
		if p.Old != "" {
			text = strings.ReplaceAll(text, p.Old, p.New)
		}
	}
	return text + r.Append
}

// Pick is a list a plugin asks a window to show, for you to choose from:
// in tabs, filtered as you type, each action on its own key. It's shown
// only in answer to one of the plugin's commands, soon after you ran it.
type Pick struct {
	ID      string       `json:"id"` // the plugin's name for it, given back with the choice
	Title   string       `json:"title"`
	About   string       `json:"about,omitempty"`
	Tabs    []string     `json:"tabs,omitempty"` // none is one list
	Tab     int          `json:"tab,omitzero"`   // the tab it opens on
	Items   []PickItem   `json:"items"`
	Actions []PickAction `json:"actions"`
	// Empty is what each tab says with nothing in it.
	Empty []string `json:"empty,omitempty"`
	// Session is whose message box it's about: a session's id, or "" for
	// the Prompt.
	Session string `json:"session,omitempty"`
}

// PickItem is one row of a Pick.
type PickItem struct {
	ID   string `json:"id"`
	Tab  int    `json:"tab,omitzero"`
	Text string `json:"text"`           // its first line is the row, the next two a preview
	Meta string `json:"meta,omitempty"` // on the row's right, faint
}

// PickAction is what a key does to the chosen item. Stay keeps the list
// open, without the item, as for forgetting one.
type PickAction struct {
	Key  string `json:"key"`
	Name string `json:"name"`
	Stay bool   `json:"stay,omitzero"`
}

// Picked is what was chosen from a Pick, and how.
type Picked struct {
	Plugin string `json:"plugin"`
	Pick   string `json:"pick"`
	Item   string `json:"item"`
	Action string `json:"action"` // its key
	UI     string `json:"ui"`
	// Box and Input are as in ui.command: whose box, and it whole, for
	// "input" only.
	Box   string `json:"box"`
	Input *Box   `json:"input,omitempty"`
}

// Limits on a Pick.
const (
	MaxPickItems   = 500
	MaxPickTabs    = 6
	MaxPickActions = 6
	MaxPickText    = 4000
)

// pickKeyRE is a key an action may take: enter, or a key with ctrl or
// alt, since what's typed filters the list.
var pickKeyRE = regexp.MustCompile(`^(enter|(ctrl|alt)\+(shift\+)?[a-z0-9])$`)

// CleanPick makes a pick safe to draw, or says why it can't be.
func CleanPick(p Pick) (Pick, error) {
	if !cmdRE.MatchString(p.ID) {
		return p, fmt.Errorf("pick id %q: use lowercase letters, digits and dashes", p.ID)
	}
	if len(p.Items) > MaxPickItems || len(p.Tabs) > MaxPickTabs || len(p.Actions) == 0 || len(p.Actions) > MaxPickActions {
		return p, fmt.Errorf("a pick has 1 to %d actions, at most %d tabs and %d items", MaxPickActions, MaxPickTabs, MaxPickItems)
	}
	p.Title, p.About = clip(printable(p.Title), 40), clip(printable(p.About), MaxLineLen)
	for i := range p.Tabs {
		p.Tabs[i] = clip(printable(p.Tabs[i]), 20)
	}
	for i := range p.Empty {
		p.Empty[i] = clip(printable(p.Empty[i]), MaxLineLen)
	}
	p.Tab = max(0, min(p.Tab, len(p.Tabs)-1))
	seen := map[string]bool{}
	for i, a := range p.Actions {
		if !pickKeyRE.MatchString(a.Key) || a.Key == "ctrl+c" || seen[a.Key] {
			return p, fmt.Errorf("action key %q: enter, or ctrl+ or alt+ a letter or digit, once each", a.Key)
		}
		seen[a.Key] = true
		p.Actions[i].Name = clip(printable(a.Name), 30)
	}
	for i, it := range p.Items {
		if it.ID == "" || len(it.ID) > 200 || it.Tab < 0 || it.Tab >= max(1, len(p.Tabs)) {
			return p, fmt.Errorf("item %d: needs an id, and a tab it has", i)
		}
		// The text keeps its lines, for the preview; each is made safe.
		lines := strings.Split(clip(it.Text, MaxPickText), "\n")
		for j := range lines {
			lines[j] = printable(lines[j])
		}
		p.Items[i].Text = strings.Join(lines, "\n")
		p.Items[i].Meta = clip(printable(it.Meta), 40)
	}
	return p, nil
}
