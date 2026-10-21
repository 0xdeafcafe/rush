package room

import (
	"cmp"
	"errors"
	"fmt"
	"os"
	"slices"
	"strconv"
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/events"
	"github.com/0xdeafcafe/rush/internal/fswait"
	"github.com/0xdeafcafe/rush/internal/host"
)

// turnLimit is the longest one turn may run before it's stopped.
const turnLimit = 10 * time.Minute

// promptBudget caps what one turn's prompt carries of the room, in bytes;
// a member with a small context window caps it lower (Member.Context).
const promptBudget = 60_000

type member struct {
	Member
	i       int
	c       *host.Client
	seen    int          // d.log entries already shown to it
	spoke   bool         // it has had a turn: it knows the rules
	resumed bool         // its last turn was paused: it picks it up again
	ready   string       // its latest READY
	readyN  int          // the turn number it said so at
	told    map[int]bool // d.log entries of yours handed into a turn already
	final   string       // its last word, once the room is over
	model   string       // the model it says it runs, once it has
	out     bool         // its session went away
}

type memberEv struct {
	i      int
	evs    []any
	closed bool
}

type driver struct {
	r       Room
	ms      []*member
	log     []Entry // what's been said, in order: what prompts are made of
	evs     chan memberEv
	user    chan Entry
	turns   int     // turns taken, numbering them
	userN   int     // the turn number you last spoke at
	queue   [][]int // what speaks next: members together (blind openings), else one at a time
	stop    bool
	verdict bool
	paused  bool
}

// Run is the driver of room id (rush room run): it starts the panel, runs
// turns until the room converges, hits its cap or you end it, and posts
// each member's final position. It returns once the room is over.
func Run(id string) error {
	r, err := Load(id)
	if err != nil {
		return err
	}
	if Running(id) {
		return fmt.Errorf("room %s is already running", id)
	}
	if err := os.WriteFile(pidPath(id), []byte(strconv.Itoa(os.Getpid())), 0o600); err != nil {
		return err
	}
	defer os.Remove(pidPath(id))
	d := &driver{r: r, evs: make(chan memberEv, 4096), user: make(chan Entry, 64)}
	entries, off, _ := Read(id, 0)
	if slices.ContainsFunc(entries, func(e Entry) bool { return e.Kind == End }) {
		return fmt.Errorf("room %s is over", id)
	}
	stop := make(chan struct{})
	defer close(stop)
	go d.watch(stop, off)
	events.Log("room-started", map[string]any{"room": id, "topic": truncate(r.Topic, 300), "members": len(r.Members), "rounds": r.Rounds})
	d.post(Entry{From: "room", Kind: Info, Text: "opening the room: " + r.Topic})
	for i, m := range r.Members {
		mm := &member{Member: m, i: i}
		d.ms = append(d.ms, mm)
		if err := d.open(mm); err != nil {
			events.Log("room-member-failed", map[string]any{"room": id, "member": m.Name, "err": err.Error()})
			mm.out = true
			d.post(Entry{From: "room", Kind: Info, Text: m.Name + " couldn't join: " + err.Error()})
		}
	}
	why := d.loop()
	events.Log("room-ended", map[string]any{"room": id, "why": why})
	d.post(Entry{From: "room", Kind: End, Text: why})
	for _, m := range d.ms {
		if m.c != nil {
			_ = m.c.Stop()
			_ = m.c.Close()
		}
	}
	return nil
}

// open starts m's session and follows it.
func (d *driver) open(m *member) error {
	cfg := m.Config
	cfg.Cwd = d.r.Cwd
	cfg.Prompt, cfg.Images = "", nil
	cfg.Name = "Room · " + m.Name + " · " + d.r.Topic
	cfg.Meta = map[string]string{"room": d.r.ID, "roomMember": m.Name}
	cfg.SystemPrompt = system
	cfg.Owner = os.Getpid() // the panel ends with its room
	started, err := host.Spawn(cfg)
	if err != nil {
		return err
	}
	host.LogEvent(started.ID, "spawned", map[string]any{"by": "room", "room": d.r.ID, "member": m.Name, "agent": cfg.Kind, "cwd": cfg.Cwd})
	c, err := host.Dial(started.ID)
	if err != nil {
		return err
	}
	m.c = c
	d.post(Entry{From: m.Name, Kind: Joined, ID: started.ID})
	go func() {
		var nt agent.Neutral
		if n, ok := agent.NativeAgent(); ok {
			nt = n.Neutral()
		}
		for l := range c.Lines {
			v, err := host.Decode(l)
			if err != nil || v == nil {
				continue
			}
			evs := []any{v}
			if _, ok := v.(event.Event); !ok && nt != nil {
				if es, ok := nt.Event(v); ok {
					evs = evs[:0]
					for _, e := range es {
						evs = append(evs, e)
					}
				}
			}
			d.evs <- memberEv{i: m.i, evs: evs}
		}
		d.evs <- memberEv{i: m.i, closed: true}
	}()
	return nil
}

// watch hands the driver what you post to the log.
func (d *driver) watch(stop <-chan struct{}, off int64) {
	file := fswait.File{Path: logPath(d.r.ID), Size: off}
	for {
		if !fswait.Grown(stop, []fswait.File{file}) {
			return
		}
		es, next, err := Read(d.r.ID, off)
		if err != nil {
			continue
		}
		off = next
		file.Size = next
		for _, e := range es {
			if e.From == User {
				select {
				case d.user <- e:
				case <-stop:
					return
				}
			}
		}
	}
}

func (d *driver) post(e Entry) {
	if e.At.IsZero() {
		e.At = time.Now()
	}
	if err := Post(d.r.ID, e); err != nil {
		fmt.Fprintln(os.Stderr, "room: post:", err)
	}
	if e.Kind == Say || e.Kind == Tool {
		d.log = append(d.log, e)
	}
}

func (d *driver) live() []*member {
	var out []*member
	for _, m := range d.ms {
		if !m.out {
			out = append(out, m)
		}
	}
	return out
}

// loop runs turns until the room is over, and says why it is.
func (d *driver) loop() string {
	round := 0
	for {
		d.takeUser(nil)
		if d.paused && !d.stop {
			d.waitResume()
		}
		live := d.live()
		switch {
		case d.stop:
			return "stopped by you"
		case len(live) < 2:
			if len(live) == 1 && len(d.log) > 0 {
				d.conclude("only " + live[0].Name + " is left in the room")
			}
			return "not enough agents left to argue"
		case d.verdict:
			return d.conclude("you called for the verdict")
		}
		if len(d.queue) == 0 {
			if round >= 2 && d.agreed(live) {
				return d.conclude("everyone said READY: yes in the same round")
			}
			if round >= d.r.Rounds {
				return d.conclude(fmt.Sprintf("the room hit its cap of %d rounds", d.r.Rounds))
			}
			round++
			var all []int
			for _, m := range live {
				all = append(all, m.i)
				d.queue = append(d.queue, []int{m.i})
			}
			if round == 1 {
				d.queue = [][]int{all} // blind openings: together, none seeing the others'
			}
		}
		batch := d.queue[0]
		d.queue = d.queue[1:]
		var ms []*member
		for _, i := range batch {
			if !d.ms[i].out {
				ms = append(ms, d.ms[i])
			}
		}
		if paused := d.speak(ms, round, ""); len(paused) > 0 {
			d.queue = append([][]int{paused}, d.queue...) // they go again first, in the same round
		}
	}
}

// waitResume holds the room while you've paused it: what you say is
// taken in, and nothing new starts until /resume or /stop.
func (d *driver) waitResume() {
	for d.paused && !d.stop {
		select {
		case e := <-d.user:
			d.onUser(e, nil)
		case me := <-d.evs:
			d.idleEvent(me)
		}
	}
}

// idleEvent is a member's event when it isn't speaking.
func (d *driver) idleEvent(me memberEv) {
	if me.closed {
		d.ms[me.i].out = true
		return
	}
	for _, ev := range me.evs {
		d.answer(d.ms[me.i], ev)
	}
}

// agreed is whether every live member's latest word, since you last
// spoke, was READY: yes.
func (d *driver) agreed(live []*member) bool {
	for _, m := range live {
		if m.ready != "yes" || m.readyN <= d.userN {
			return false
		}
	}
	return true
}

// takeUser takes in what you've posted; speaking are the members whose
// turns are under way.
func (d *driver) takeUser(speaking []*turn) {
	for {
		select {
		case e := <-d.user:
			d.onUser(e, speaking)
		default:
			return
		}
	}
}

func (d *driver) onUser(e Entry, speaking []*turn) {
	if e.Kind == Control {
		switch e.Text {
		case "stop":
			d.stop = true
		case "verdict":
			d.verdict, d.paused = true, false // the final positions go ahead, paused or not
			d.guide(speaking, "", -1, "[The user has called for the verdict]: finish this turn now, in a sentence or two.")
		case "pause":
			if !d.paused {
				d.paused = true
				for _, t := range speaking {
					if !t.done && !t.paused {
						t.paused = true
						_ = t.m.c.Interrupt()
					}
				}
				d.post(Entry{From: "room", Kind: Info, Text: "paused: no turn starts until /resume"})
			}
		case "resume":
			if d.paused {
				d.paused = false
				d.post(Entry{From: "room", Kind: Info, Text: "resumed"})
			}
		}
		return
	}
	if e.Kind != Say {
		return
	}
	d.log = append(d.log, e)
	d.userN = d.turns
	if e.To != "" {
		for _, m := range d.ms {
			if strings.EqualFold(m.Name, e.To) && !m.out && !slices.ContainsFunc(speaking, func(t *turn) bool { return t.m == m }) {
				// Straight after this turn, in place of its turn this round.
				for i, b := range d.queue {
					d.queue[i] = slices.DeleteFunc(b, func(i int) bool { return i == m.i })
				}
				d.queue = append([][]int{{m.i}}, slices.DeleteFunc(d.queue, func(b []int) bool { return len(b) == 0 })...)
			}
		}
	}
	if !d.paused {
		d.guide(speaking, e.To, len(d.log)-1, "[The user, while you work]: "+e.Text+"\n(Answer the user first.)")
	}
}

// guide hands text into the turns under way whose agents take a message
// mid-turn, those it's to; the rest read it at their next turn.
func (d *driver) guide(speaking []*turn, to string, entry int, text string) {
	for _, t := range speaking {
		if t.done || t.paused || to != "" && !strings.EqualFold(to, t.m.Name) || !agent.Supports(agent.Kind(t.m.Config.Kind), agent.FeatureGuide) {
			continue
		}
		if t.m.c.SendGuide(text, nil) == nil && entry >= 0 {
			if t.m.told == nil {
				t.m.told = map[int]bool{}
			}
			t.m.told[entry] = true
		}
	}
}

// turn is one member's turn under way.
type turn struct {
	m            *member
	n, round     int
	final        string // why the room is over, when this is its last word
	said, failed string
	ended, done  bool
	paused       bool
	posted       map[string]string // tool calls and results posted, by ID: agents repeat them
	started      time.Time
}

// speak has ms speak at once (one, past the openings), and posts what
// they say; it returns those paused part way, to go again.
func (d *driver) speak(ms []*member, round int, final string) []int {
	var ts []*turn
	for _, m := range ms {
		d.turns++
		t := &turn{m: m, n: d.turns, round: round, final: final, posted: map[string]string{}, started: time.Now()}
		prompt := d.prompt(m, round, final) // all made before any is sent: openings stay blind
		m.seen, m.spoke, m.resumed = len(d.log), true, false
		if final == "" {
			d.post(Entry{From: m.Name, Kind: Turn, Round: round})
		}
		if err := m.c.Send(prompt); err != nil {
			d.fail(m, err)
			continue
		}
		ts = append(ts, t)
	}
	tick := time.NewTicker(10 * time.Second)
	defer tick.Stop()
	for slices.ContainsFunc(ts, func(t *turn) bool { return !t.done }) {
		select {
		case e := <-d.user:
			d.onUser(e, ts)
			if d.stop {
				for _, t := range ts {
					if !t.done {
						_ = t.m.c.Interrupt()
					}
				}
				return nil
			}
		case <-tick.C:
			for _, t := range ts {
				if !t.done && !t.paused && time.Since(t.started) > turnLimit {
					_ = t.m.c.Interrupt()
					d.post(Entry{From: "room", Kind: Info, Text: t.m.Name + " ran past " + turnLimit.String() + " and was stopped"})
					t.said = strings.TrimSpace(t.said + "\n\n(cut off)")
					d.finish(t)
				}
			}
		case me := <-d.evs:
			i := slices.IndexFunc(ts, func(t *turn) bool { return t.m.i == me.i })
			if i < 0 || ts[i].done {
				d.idleEvent(me)
				continue
			}
			d.event(ts[i], me)
		}
	}
	var paused []int
	for _, t := range ts {
		if t.paused && !t.m.out {
			t.m.resumed = true
			paused = append(paused, t.m.i)
		}
	}
	return paused
}

// event takes one batch of events from t's member.
func (d *driver) event(t *turn, me memberEv) {
	m := t.m
	if me.closed {
		t.done = true
		d.fail(m, errors.New("its session went away"))
		return
	}
	for _, ev := range me.evs {
		switch e := ev.(type) {
		case event.Message:
			if e.Parent != "" {
				continue // a subagent's: its steps stay in its session
			}
			// Words with tool calls are on the way to an answer: a note
			// before the calls. The last words without any are the turn's.
			var text strings.Builder
			calls := false
			for _, p := range e.Parts {
				if p.Kind == event.Text && e.Role == "assistant" {
					text.WriteString(p.Text)
				}
				calls = calls || p.Kind == event.ToolCall
			}
			if s := strings.TrimSpace(text.String()); s != "" || calls {
				if t.said != "" {
					d.post(Entry{From: m.Name, Kind: Note, Text: t.said})
				}
				if t.said = s; calls && s != "" {
					d.post(Entry{From: m.Name, Kind: Note, Text: s})
					t.said = ""
				}
			}
			for _, p := range e.Parts {
				switch {
				case p.Kind == event.ToolCall && p.Call != nil:
					d.postCall(t, *p.Call)
				case p.Kind == event.ToolResult && p.Output != nil && t.posted["done "+p.Output.CallID] == "":
					t.posted["done "+p.Output.CallID] = "y"
					d.post(Entry{From: m.Name, Kind: Done, ID: p.Output.CallID, Failed: p.Output.IsError})
				}
			}
		case event.CallUpdated:
			d.postCall(t, e.Call)
		case event.TurnEnd:
			t.ended = true
			if t.said == "" {
				t.said = strings.TrimSpace(e.Text)
			}
			if e.Reason == "error" {
				t.failed = cmp.Or(e.Err, "the turn failed")
			}
		case host.InfoEvent:
			i := e.Info
			if i.Model != "" && i.Model != m.model && m.Config.Model == "" {
				m.model = i.Model
				d.post(Entry{From: m.Name, Kind: Uses, Text: i.Model}) // the label names the model it actually runs
			}
			if i.State == "working" {
				t.ended = false
			}
			busy := i.Retry != nil && !i.Retry.GaveUp || len(i.Background) > 0
			if t.ended && (i.State == "idle" && len(i.Queue) == 0 && !busy || i.Limit != nil) || i.State == "stopped" {
				if t.failed != "" && t.said == "" && !t.paused {
					d.post(Entry{From: "room", Kind: Info, Text: m.Name + "'s turn failed: " + t.failed})
				}
				d.finish(t)
			}
		case host.ErrorEvent:
			d.post(Entry{From: "room", Kind: Info, Text: m.Name + ": " + e.Error})
		default:
			d.answer(m, ev)
		}
	}
}

// finish posts t's turn: what it said, its words so far if you paused it,
// or its final position.
func (d *driver) finish(t *turn) {
	t.done = true
	m := t.m
	text, ready := Ready(t.said)
	switch {
	case t.paused:
		if text != "" {
			d.post(Entry{From: m.Name, Kind: Note, Text: text})
		}
		d.post(Entry{From: m.Name, Kind: Paused, Text: m.Name + " was paused mid-turn: it picks it up again on /resume"})
	case t.final != "":
		m.final = strings.Join(strings.Fields(cmp.Or(text, "(said nothing)")), " ")
	default:
		m.ready, m.readyN = ready, t.n
		d.post(Entry{From: m.Name, Kind: Say, Text: cmp.Or(text, "(said nothing)"), Ready: ready, Round: t.round})
	}
}

func (d *driver) fail(m *member, err error) {
	events.Log("room-member-left", map[string]any{"room": d.r.ID, "member": m.Name, "err": err.Error()})
	m.out = true
	d.post(Entry{From: "room", Kind: Info, Text: m.Name + " left the room: " + err.Error()})
}

func (d *driver) postCall(t *turn, c tool.Call) {
	detail := c.Input.Command
	if detail == "" {
		detail = c.Input.Path
	}
	if detail == "" {
		detail = cmp.Or(c.Input.Pattern, c.Input.URL)
	}
	if i := strings.IndexByte(detail, '\n'); i >= 0 {
		detail = detail[:i] + " …"
	}
	if detail == "" && c.Title == "" && c.Kind != tool.Other && c.Kind != tool.MCP {
		return // announced before its input is known: it comes again with it
	}
	c.Raw, c.Input.Content, c.Input.Edits = nil, "", nil // a room changes no files: what they would have written stays out of its log
	e := Entry{From: t.m.Name, Kind: Tool, ID: c.ID, Text: tool.Doing(c), Detail: truncate(detail, 300), Call: &c}
	if key := e.Text + "\x00" + e.Detail; t.posted[c.ID] != key {
		t.posted[c.ID] = key
		d.post(e)
	}
}

// answer settles what m asks: reading and running are allowed, changing
// files is not, and questions go back unanswered.
func (d *driver) answer(m *member, ev any) {
	if m.c == nil {
		return
	}
	switch e := ev.(type) {
	case event.Approval:
		if e.Call.Kind.Changes() {
			events.Log("room-denied", map[string]any{"room": d.r.ID, "member": m.Name, "call": truncate(tool.Doing(e.Call), 300), "why": "a room changes no files"})
			_ = m.c.Deny(e.ID, "This is a discussion room: don't change files. Say what you'd change instead.", false)
			d.post(Entry{From: "room", Kind: Info, Text: m.Name + " wasn't allowed to " + tool.Doing(e.Call)})
			return
		}
		_ = m.c.Allow(e.ID, nil, false)
	case event.Question:
		_ = m.c.Deny(e.ID, "No one answers questions mid-turn in a room: state your assumption and carry on.", false)
	}
}

// conclude has every member give its final position, and posts them as
// the room's verdict: one line each, agreeing or not.
func (d *driver) conclude(why string) string {
	pending := d.live()
	events.Log("room-concluding", map[string]any{"room": d.r.ID, "why": why, "members": len(pending)})
	d.post(Entry{From: "room", Kind: Info, Text: "the discussion is over (" + why + "): each agent gives its final position"})
	for len(pending) > 0 && !d.stop {
		paused := d.speak(pending, 0, why)
		pending = nil
		for _, i := range paused {
			pending = append(pending, d.ms[i])
		}
		if len(pending) > 0 {
			d.waitResume()
		}
	}
	var lines []string
	for _, m := range d.ms {
		if m.final != "" {
			lines = append(lines, m.Name+": "+m.final)
		}
	}
	if len(lines) > 0 {
		d.post(Entry{From: "room", Kind: Verdict, Text: strings.Join(lines, "\n"), Detail: why})
	}
	if d.stop {
		return "stopped by you"
	}
	return why
}

// prompt is what m is sent for its turn: the room's rules the first time,
// then everything said since its last turn.
func (d *driver) prompt(m *member, round int, final string) string {
	var b strings.Builder
	if !m.spoke {
		fmt.Fprintf(&b, "You are %s, one of %d AI agents in a rush room: a group discussion the user watches live and can join at any time.\n\n", m.Name, len(d.ms))
		b.WriteString("The panel:\n")
		for _, o := range d.ms {
			fmt.Fprintf(&b, "- %s (%s)\n", o.Name, o.Label())
		}
		fmt.Fprintf(&b, "\nTopic: %s\nWorking folder: %s\n", d.r.Topic, d.r.Cwd)
		b.WriteString(ifSet("\nThe user opened this room from a chat with an agent. The latest of that chat, for context:\n\n", d.r.Brief))
		fmt.Fprintf(&b, "\n%s\n", fmt.Sprintf(rules, d.r.Rounds))
	}
	delta, toYou, fromUser := d.since(m)
	if delta != "" {
		if !m.spoke {
			b.WriteString("\nSo far in the room:\n")
		} else {
			b.WriteString("\nNew in the room since your last turn:\n")
		}
		b.WriteString(delta)
	}
	b.WriteString("\n")
	if final != "" {
		fmt.Fprintf(&b, "The discussion is over: %s. State your own final position on the topic in one line, as you hold it now, whether or not the others agree. No tools, and no READY line.\n", final)
		return b.String()
	}
	if m.resumed {
		b.WriteString("The user paused the room in the middle of your last turn and has now resumed it: pick your turn up again.\n")
	}
	switch {
	case toYou:
		b.WriteString("The user spoke to you directly: answer the user first.\n")
	case fromUser:
		b.WriteString("The user spoke: answer the user first, then the others.\n")
	}
	if round == 1 {
		fmt.Fprintf(&b, "Round 1: openings. Everyone writes theirs at once, without seeing the others'; give your own view, %s. End with READY: yes or READY: no.\n", m.Name)
		return b.String()
	}
	fmt.Fprintf(&b, "It's your turn, %s (round %d of at most %d). Reply to the others by name, and end with READY: yes or READY: no.\n", m.Name, round, d.r.Rounds)
	return b.String()
}

// since is the room since m's last turn, as m reads it: whether you spoke
// to it, and whether you spoke at all.
func (d *driver) since(m *member) (string, bool, bool) {
	var parts []string
	toYou, fromUser := false, false
	for i, e := range d.log[m.seen:] {
		switch {
		case e.From == m.Name || m.told[m.seen+i]:
			continue
		case e.From == User:
			fromUser = true
			who := "The user"
			if e.To != "" {
				who += " (to " + e.To + ")"
				toYou = toYou || strings.EqualFold(e.To, m.Name)
			}
			parts = append(parts, fmt.Sprintf("\n%s:\n%s\n", who, e.Text))
		case e.Kind == Tool:
			parts = append(parts, fmt.Sprintf("  (%s ran a tool: %s%s)\n", e.From, e.Text, ifSet(" — ", e.Detail)))
		case e.Kind == Say:
			parts = append(parts, fmt.Sprintf("\n%s:\n%s\n%s", e.From, e.Text, ifSet("READY: ", e.Ready)))
		}
	}
	// The newest is kept whole; the oldest goes first when it's long.
	budget := d.budget()
	total, from := 0, len(parts)
	for from > 0 && total+len(parts[from-1]) <= budget {
		from--
		total += len(parts[from])
	}
	if from == len(parts) && from > 0 { // even the newest is too long: its tail
		last := parts[from-1]
		parts[from-1] = "[shortened] " + strings.ToValidUTF8(last[max(0, len(last)-budget):], "")
		from--
	}
	out := strings.Join(parts[from:], "")
	if from > 0 {
		out = "(earlier turns left out for length)\n" + out
	}
	return out, toYou, fromUser
}

// budget is promptBudget, or less where a member's context window is
// small: about a byte and a half per token of it, less the rules.
func (d *driver) budget() int {
	b := promptBudget
	for _, m := range d.ms {
		if m.Context > 0 {
			b = min(b, max(2048, m.Context*3/2-4096))
		}
	}
	return b
}

func ifSet(lead, s string) string {
	if s == "" {
		return ""
	}
	return lead + s
}

func truncate(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return strings.ToValidUTF8(s[:n], "") + "…"
}

const system = `You are taking part in a rush room: a live group discussion between AI agents of different models, which the user watches and can join at any time. Everything you write is visible to everyone. Hold real opinions: test claims, challenge the others by name when you think they're wrong, and change your mind only when persuaded, saying what persuaded you. You may read files and run read-only commands to check facts; don't edit or create files. Keep each turn short.`

const rules = `How the room works:
- Round 1 is openings: everyone writes theirs at once, without seeing the others'. From round 2 you take turns, and each turn you're shown everything said since your last, including the tools the others ran.
- Argue honestly. Form your own view, check claims against the code or by running read-only commands when it matters, and challenge the others by name when you think they're wrong. Don't agree to be polite.
- Don't edit or create files: this is a discussion.
- The user may speak at any time. When they do, answer them first.
- Keep each turn to a few short paragraphs.
- End every turn with exactly one last line: "READY: yes" when you hold a position you'd sign and nothing the others said changes it, or "READY: no" while something is still unsettled.
- The room ends when everyone says READY: yes in the same round (from round 2), or after %d rounds; then each of you states a final position in one line.`
