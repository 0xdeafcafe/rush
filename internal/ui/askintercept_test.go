package ui

import (
	"context"
	"encoding/json/jsontext"
	"errors"
	"fmt"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/hooks"
	"github.com/0xdeafcafe/rush/internal/jsonx"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

// askPlugin is a plugin that asks before a message holding a token
// goes: y stores the token and puts a reference in its place, with a line
// on using it at the end; n lets the token go as it is. A failed store
// asks again, whether to send it as it is.
type askPlugin struct {
	mu     sync.Mutex
	stored map[string]string
	addErr error
	asked  map[string]askQ
	letGo  map[string]bool
	n      int
	// named has it ask for the secret's name after a y, in a window that
	// shows an input.
	named bool
}

type askQ struct {
	value, name string
	failed      bool
	// naming is the question for its name, err why the last one typed
	// wasn't taken.
	naming bool
	err    string
}

var askNameRE = regexp.MustCompile(`^[A-Z_][A-Z0-9_]*$`)

var tokenRE = regexp.MustCompile(`sk-proj-[A-Za-z0-9]{40,}|ghp_[A-Za-z0-9]{36}`)

func askRef(name string) string { return "{{secret:" + name + "}}" }

func askUsage(name string) string { return "(" + askRef(name) + " is a stored secret.)" }

func (f *askPlugin) saved(name string) string {
	f.mu.Lock()
	defer f.mu.Unlock()
	return f.stored[name]
}

func (f *askPlugin) next(text string) (value, name string) {
	f.mu.Lock()
	defer f.mu.Unlock()
	for _, v := range tokenRE.FindAllString(text, -1) {
		if f.letGo[v] {
			continue
		}
		if strings.HasPrefix(v, "ghp_") {
			return v, "GITHUB_TOKEN"
		}
		return v, "OPENAI_API_KEY"
	}
	return "", ""
}

func (f *askPlugin) ask(q askQ, changes plugin.InterceptResult) plugin.InterceptResult {
	f.mu.Lock()
	f.n++
	id := strconv.Itoa(f.n)
	if f.asked == nil {
		f.asked = map[string]askQ{}
	}
	f.asked[id] = q
	f.mu.Unlock()
	changes.Action, changes.ID = "ask", id
	if q.failed {
		changes.Question, changes.Detail = "Couldn't store the secret", f.addErr.Error()
		changes.Choices = []plugin.AskChoice{{Key: "y", Label: "send as is"}, {Key: "n", Label: "back to the box", Esc: true}}
		return changes
	}
	if q.naming {
		changes.Question, changes.Detail = "Name the secret", "it goes as "+askRef("NAME")
		changes.Input = &plugin.AskLine{Value: q.name, Error: q.err, Enter: "save"}
		changes.Choices = []plugin.AskChoice{{Key: "b", Label: "back", Esc: true}}
		return changes
	}
	if f.named {
		changes.Question = "Save this secret?"
		changes.Detail = fmt.Sprintf("your message has a token (%s… %d chars)", q.value[:4], len(q.value))
		changes.Choices = []plugin.AskChoice{{Key: "y", Label: "save it", Enter: true}, {Key: "n", Label: "send as is", Esc: true}}
		return changes
	}
	changes.Question = "Save as secret " + q.name + "?"
	changes.Detail = fmt.Sprintf("your message has a token (%s… %d chars) · saved, it goes as %s", q.value[:4], len(q.value), askRef(q.name))
	changes.Choices = []plugin.AskChoice{{Key: "y", Label: "save it", Enter: true}, {Key: "n", Label: "send as is", Esc: true}}
	return changes
}

// then is changes to text, asking about the next token left in it.
func (f *askPlugin) then(text string, changes plugin.InterceptResult) plugin.InterceptResult {
	if v, name := f.next(changes.Apply(text)); v != "" {
		return f.ask(askQ{value: v, name: name}, changes)
	}
	if len(changes.Replace) == 0 && changes.Append == "" {
		return plugin.InterceptResult{Action: "allow"}
	}
	changes.Action = "rewrite"
	return changes
}

func (f *askPlugin) handle(_ context.Context, method string, params jsontext.Value) (any, error) {
	switch method {
	case "ui.intercept":
		var in plugin.Intercept
		_ = jsonx.Unmarshal(params, &in)
		return f.then(in.Text, plugin.InterceptResult{}), nil
	case "ui.intercept.answer":
		var in plugin.InterceptAnswer
		_ = jsonx.Unmarshal(params, &in)
		f.mu.Lock()
		q := f.asked[in.ID]
		delete(f.asked, in.ID)
		f.mu.Unlock()
		switch {
		case q.failed && in.Key == "y", !q.failed && in.Key == "n":
			f.mu.Lock()
			if f.letGo == nil {
				f.letGo = map[string]bool{}
			}
			f.letGo[q.value] = true
			f.mu.Unlock()
			return f.then(in.Text, plugin.InterceptResult{}), nil
		case q.failed:
			return plugin.InterceptResult{Action: "block", Reason: "not sent; it's still in the box"}, nil
		case q.naming && in.Key == "b":
			q.naming, q.err = false, ""
			return f.ask(q, plugin.InterceptResult{}), nil
		case q.naming:
			q.name, q.err = in.Value, ""
			if !askNameRE.MatchString(in.Value) {
				q.err = "a name is capitals, digits and _"
			} else if was := f.saved(in.Value); was != "" && was != q.value {
				q.err = in.Value + " is already stored: pick another name"
			}
			if q.err != "" {
				return f.ask(q, plugin.InterceptResult{}), nil
			}
		case f.named && slices.Contains(in.Asks, plugin.AskInput):
			q.naming = true
			return f.ask(q, plugin.InterceptResult{}), nil
		}
		if f.addErr != nil {
			q.failed = true
			return f.ask(q, plugin.InterceptResult{}), nil
		}
		f.mu.Lock()
		if f.stored == nil {
			f.stored = map[string]string{}
		}
		f.stored[q.name] = q.value
		f.mu.Unlock()
		replaced := strings.ReplaceAll(in.Text, q.value, askRef(q.name))
		sep := "\n\n"
		if last := replaced[strings.LastIndexByte(replaced, '\n')+1:]; strings.HasPrefix(last, "({{secret:") {
			sep = "\n"
		}
		return f.then(in.Text, plugin.InterceptResult{Replace: []plugin.Replacement{{Old: q.value, New: askRef(q.name)}}, Append: sep + askUsage(q.name)}), nil
	}
	return map[string]any{}, nil
}

// askHooks are a window's plugins: f, behind a broker that does what
// plugind does with one plugin's answers.
func askHooks(t *testing.T, f *askPlugin) *hooks.Client {
	t.Helper()
	const name = "secrets"
	c := hooks.Over(plugin.UIState{Plugins: []plugin.UIPlugin{{Name: name, UI: []string{plugin.UIInput, plugin.UIIntercept}}}},
		func(ctx context.Context, method string, params jsontext.Value) (any, error) {
			if method != "ui.intercept" && method != "ui.intercept.answer" {
				return map[string]any{}, nil
			}
			var in plugin.Intercept
			_ = jsonx.Unmarshal(params, &in)
			out, err := f.handle(ctx, method, params)
			if err != nil {
				return nil, err
			}
			r := out.(plugin.InterceptResult)
			if r.Action == "ask" {
				if err := plugin.CleanAsk(&r); err != nil {
					return nil, err
				}
			}
			if r.Action == "ask" || r.Action == "rewrite" {
				r.Text = r.Apply(in.Text)
			}
			r.Plugin = name
			return r, nil
		})
	c.Start()
	t.Cleanup(c.Close)
	for deadline := time.Now().Add(2 * time.Second); !c.Intercepts(); time.Sleep(5 * time.Millisecond) {
		if time.Now().After(deadline) {
			t.Fatal("the window never reached its plugins")
		}
	}
	return c
}

// A fake but real-looking key, built at run time.
var pastedKey = "sk-proj-" + strings.Repeat("Ab3dEf7gHi2jKlMn", 3)

// land runs cmd, the plugins' say, and hands what it brings back to the
// window, returning what follows.
func land(t *testing.T, m *Model, cmd tea.Cmd) tea.Cmd {
	t.Helper()
	if cmd == nil {
		t.Fatal("nothing went to the plugins")
	}
	msg, ok := cmd().(interceptedMsg)
	if !ok {
		t.Fatal("the plugins' say didn't come back")
	}
	return m.onIntercepted(msg)
}

func boxWithKey(t *testing.T, v *askPlugin) (*Model, *hostConn) {
	m, c := infoModel(t)
	m.hooks = askHooks(t, v)
	c.input = []rune("deploy with " + pastedKey + " please")
	return m, c
}

func TestPastedKeyAsksToSaveIt(t *testing.T) {
	v := &askPlugin{}
	m, c := boxWithKey(t, v)
	land(t, m, m.sendPane(c, false))
	if m.confirm == nil || m.confirm.question != "Save as secret OPENAI_API_KEY?" {
		t.Fatalf("want the save question, got %+v", m.confirm)
	}
	if len(c.sending) != 0 {
		t.Fatal("nothing goes before you answer")
	}
	if strings.Contains(m.confirm.detail, pastedKey) || !strings.Contains(m.confirm.detail, "sk-p… 56 chars") ||
		!strings.Contains(m.confirm.detail, "it goes as {{secret:OPENAI_API_KEY}}") {
		t.Errorf("the question names the key without showing it: %s", m.confirm.detail)
	}
	if keys := stripAnsi(m.confirm.keys()); keys != "y save it   n/esc send as is   ctrl+c cancel" {
		t.Errorf("keys %q", keys)
	}

	land(t, m, m.confirmKey("y"))
	if v.saved("OPENAI_API_KEY") != pastedKey {
		t.Fatalf("saved %v", v.stored)
	}
	want := "deploy with {{secret:OPENAI_API_KEY}} please\n\n" + askUsage("OPENAI_API_KEY")
	if sent := sentText(t, c); sent != want {
		t.Errorf("sent %q, want %q", sent, want)
	}
	if len(c.input) != 0 || c.intercepting {
		t.Error("a sent box is empty and no longer asked about")
	}
}

func TestEnterSavesToo(t *testing.T) {
	v := &askPlugin{}
	m, c := boxWithKey(t, v)
	land(t, m, m.sendPane(c, false))
	land(t, m, m.confirmKey("enter"))
	if v.saved("OPENAI_API_KEY") != pastedKey || !strings.Contains(sentText(t, c), "{{secret:OPENAI_API_KEY}}") {
		t.Fatalf("enter should save and send: %v", v.stored)
	}
}

func TestNoAndEscSendUnchanged(t *testing.T) {
	for _, key := range []string{"n", "esc"} {
		v := &askPlugin{}
		m, c := boxWithKey(t, v)
		land(t, m, m.sendPane(c, false))
		land(t, m, m.confirmKey(key))
		if len(v.stored) != 0 {
			t.Errorf("%s saved %v", key, v.stored)
		}
		if got := sentText(t, c); got != "deploy with "+pastedKey+" please" {
			t.Errorf("%s sent %q", key, got)
		}
	}
}

func TestCtrlCCancelsTheSend(t *testing.T) {
	v := &askPlugin{}
	m, c := boxWithKey(t, v)
	land(t, m, m.sendPane(c, false))
	if cmd := m.confirmKey("ctrl+c"); cmd != nil || m.confirm != nil {
		t.Fatal("ctrl+c closes the question and asks nothing more")
	}
	if len(c.sending) != 0 || string(c.input) != "deploy with "+pastedKey+" please" || c.intercepting || len(v.stored) != 0 {
		t.Fatalf("ctrl+c leaves the box: sent %d, box %q", len(c.sending), string(c.input))
	}
}

func TestFailedSaveNeverSendsOnItsOwn(t *testing.T) {
	v := &askPlugin{addErr: errors.New("the master is not running")}
	m, c := boxWithKey(t, v)
	land(t, m, m.sendPane(c, false))
	land(t, m, m.confirmKey("y"))
	if len(c.sending) != 0 {
		t.Fatal("a failed save sent the message")
	}
	if m.confirm == nil || m.confirm.question != "Couldn't store the secret" || !strings.Contains(m.confirm.detail, "the master is not running") {
		t.Fatalf("want the error shown, got %+v", m.confirm)
	}
	if m.confirmKey("enter"); len(c.sending) != 0 || m.confirm == nil {
		t.Fatal("enter alone must not send the raw key after a failed save")
	}
	land(t, m, m.confirmKey("esc"))
	if len(c.sending) != 0 || string(c.input) != "deploy with "+pastedKey+" please" {
		t.Fatalf("esc goes back to the message: sent %d, box %q", len(c.sending), string(c.input))
	}

	// Sent again, it asks again, and this time y sends it as it is.
	land(t, m, m.sendPane(c, false))
	land(t, m, m.confirmKey("y"))
	land(t, m, m.confirmKey("y"))
	if got := sentText(t, c); got != "deploy with "+pastedKey+" please" {
		t.Errorf("sent %q", got)
	}
}

func TestTwoSecretsAskOneAfterTheOther(t *testing.T) {
	v := &askPlugin{}
	m, c := boxWithKey(t, v)
	gh := "ghp_" + strings.Repeat("Zq7Wx3Rt9", 4)
	c.input = []rune(pastedKey + " and " + gh)
	land(t, m, m.sendPane(c, false))
	land(t, m, m.confirmKey("y"))
	if m.confirm == nil || m.confirm.question != "Save as secret GITHUB_TOKEN?" || len(c.sending) != 0 {
		t.Fatalf("want the second question, got %+v", m.confirm)
	}
	if box := string(c.input); strings.Contains(box, pastedKey) || !strings.HasPrefix(box, "{{secret:OPENAI_API_KEY}} and ") {
		t.Errorf("a saved secret leaves the box, so drafts never keep it: %q", box)
	}
	land(t, m, m.confirmKey("y"))
	sent := sentText(t, c)
	if strings.Contains(sent, pastedKey) || strings.Contains(sent, gh) ||
		!strings.HasPrefix(sent, "{{secret:OPENAI_API_KEY}} and {{secret:GITHUB_TOKEN}}") ||
		!strings.Contains(sent, askUsage("OPENAI_API_KEY")+"\n"+askUsage("GITHUB_TOKEN")) {
		t.Errorf("sent %q", sent)
	}
}

func TestOrdinaryMessageIsNotAsked(t *testing.T) {
	v := &askPlugin{}
	m, c := boxWithKey(t, v)
	c.input = []rune("set OPENAI_API_KEY=sk-1234 or <your-key> and commit 51d07b547d0a8f3e2c1b9d4a6e7f8091a2b3c4d5")
	land(t, m, m.sendPane(c, false))
	if m.confirm != nil || len(c.sending) != 1 {
		t.Fatalf("an ordinary message goes straight out: confirm %+v", m.confirm)
	}
}

func TestPromptAsksBeforeStarting(t *testing.T) {
	v := &askPlugin{}
	m, _ := infoModel(t)
	m.hooks = askHooks(t, v)
	m.input = []rune("use " + pastedKey)
	land(t, m, m.submit())
	if m.confirm == nil || m.confirm.question != "Save as secret OPENAI_API_KEY?" {
		t.Fatalf("want the save question, got %+v", m.confirm)
	}
	if string(m.input) != "use "+pastedKey {
		t.Fatal("the Prompt keeps its message while you're asked")
	}
	cmd := m.confirmKey("y")
	if !m.promptIntercepting || m.submit() != nil {
		t.Fatal("the Prompt doesn't send while the plugin acts on the answer")
	}
	msg := cmd().(interceptedMsg)
	if !msg.prompt || msg.r.Action != "rewrite" {
		t.Fatalf("y came back as %+v", msg)
	}
	m.onIntercepted(msg)
	if v.saved("OPENAI_API_KEY") != pastedKey || len(m.input) != 0 || m.promptIntercepting {
		t.Fatalf("saved %v, Prompt %q", v.stored, string(m.input))
	}
}

// A secret inside a long paste is saved the same: the chip stays a chip,
// and its text goes out with the reference.
func TestSecretInAPaste(t *testing.T) {
	v := &askPlugin{}
	m, c := infoModel(t)
	m.hooks = askHooks(t, v)
	paste := "line one\nOPENAI_API_KEY=" + pastedKey + "\nline three"
	c.input = []rune("my env:\n" + c.pastes.add(paste))
	chip := string(c.input)
	land(t, m, m.sendPane(c, false))
	msg := m.confirmKey("y")().(interceptedMsg)
	// The change goes into the box in place, so the chip stays a chip.
	b, _ := m.interceptBox(msg)
	b.rewrite(msg.r)
	if !strings.HasPrefix(string(c.input), chip) || len(c.pastes.text) != 1 {
		t.Fatalf("box %q, pastes %d", string(c.input), len(c.pastes.text))
	}
	for _, p := range c.pastes.text {
		if strings.Contains(p, pastedKey) || !strings.Contains(p, "{{secret:OPENAI_API_KEY}}") {
			t.Fatalf("paste %q", p)
		}
	}
	msg.was, msg.r = string(c.input), plugin.InterceptResult{Action: "allow"}
	m.onIntercepted(msg)
	sent := sentText(t, c)
	if strings.Contains(sent, pastedKey) || !strings.Contains(sent, "OPENAI_API_KEY={{secret:OPENAI_API_KEY}}") {
		t.Errorf("sent %q", sent)
	}
}

// enter presses enter in the window, and returns what follows.
func enter(m *Model) tea.Cmd {
	k, _ := keyOf("enter")
	return m.key(k)
}

// named is a box with a key in it, y said to saving it: the question for
// its name is up.
func named(t *testing.T, v *askPlugin) (*Model, *hostConn) {
	t.Helper()
	v.named = true
	m, c := boxWithKey(t, v)
	land(t, m, m.sendPane(c, false))
	if m.confirm == nil || m.confirm.question != "Save this secret?" || m.confirm.line != nil {
		t.Fatalf("want the save question with no name in it, got %+v", m.confirm)
	}
	land(t, m, m.confirmKey("y"))
	if m.confirm == nil || m.confirm.line == nil {
		t.Fatalf("want the name asked for, got %+v", m.confirm)
	}
	return m, c
}

// Saying y asks for the secret's name in a line that starts with the one
// suggested, edited in place; enter saves under what's there.
func TestYesAsksForTheName(t *testing.T) {
	v := &askPlugin{}
	m, c := named(t, v)
	l := m.confirm.line
	if string(l.buf) != "OPENAI_API_KEY" || l.pos != len(l.buf) || len(v.stored) != 0 || len(c.sending) != 0 {
		t.Fatalf("the line starts as the suggested name, nothing saved yet: %q at %d", string(l.buf), l.pos)
	}
	if keys := stripAnsi(m.confirm.keys()); keys != "enter save   esc back   ctrl+c cancel" {
		t.Errorf("keys %q", keys)
	}
	body := stripAnsi(strings.Join(m.confirmBody(60), "\n"))
	if !strings.Contains(body, "Name the secret") || !strings.Contains(body, "❯ OPENAI_API_KEY▏") {
		t.Errorf("the box shows the line:\n%s", body)
	}
	// Letters are typed, y and n included; the arrows move in the line.
	pressKeys(m, "ctrl+w", "ctrl+w", "ctrl+w", "S", "T", "R", "Y", "P", "E", "_", "n", "backspace", "K", "E", "Y", "home", "M", "Y", "_", "end", "2", "left", "delete")
	if got := string(m.confirm.line.buf); got != "MY_STRYPE_KEY" {
		t.Fatalf("typed %q", got)
	}
	m.confirm.line.insert("_LIVE\n")
	land(t, m, enter(m))
	if v.saved("MY_STRYPE_KEY_LIVE") != pastedKey || len(v.stored) != 1 {
		t.Fatalf("saved %v", v.stored)
	}
	want := "deploy with {{secret:MY_STRYPE_KEY_LIVE}} please\n\n" + askUsage("MY_STRYPE_KEY_LIVE")
	if sent := sentText(t, c); sent != want {
		t.Errorf("sent %q, want %q", sent, want)
	}
}

// A name the plugin won't take keeps the question up, with why under the
// line and what was typed still in it.
func TestBadNameStaysOpen(t *testing.T) {
	v := &askPlugin{stored: map[string]string{"TAKEN": "another value"}}
	m, c := named(t, v)
	for _, bad := range []struct{ typed, why string }{
		{"my key", "a name is capitals, digits and _"},
		{"TAKEN", "TAKEN is already stored: pick another name"},
	} {
		pressKeys(m, "ctrl+u")
		for _, r := range bad.typed {
			pressKeys(m, strings.Replace(string(r), " ", "space", 1))
		}
		land(t, m, enter(m))
		if m.confirm == nil || m.confirm.line == nil || m.confirm.line.err != bad.why || string(m.confirm.line.buf) != bad.typed {
			t.Fatalf("%q: want it asked again with why, got %+v", bad.typed, m.confirm)
		}
		if len(c.sending) != 0 || len(v.stored) != 1 {
			t.Fatalf("%q: nothing is saved or sent: %v", bad.typed, v.stored)
		}
		if body := stripAnsi(strings.Join(m.confirmBody(60), "\n")); !strings.Contains(body, bad.why) {
			t.Errorf("%q: the box says why:\n%s", bad.typed, body)
		}
	}
	// Typing takes the reason away; a good name then saves.
	pressKeys(m, "_", "2")
	if m.confirm.line.err != "" {
		t.Error("the reason stays after the line changed")
	}
	land(t, m, enter(m))
	if v.saved("TAKEN_2") != pastedKey || !strings.Contains(sentText(t, c), "{{secret:TAKEN_2}}") {
		t.Fatalf("saved %v", v.stored)
	}
}

// esc in the name goes back to the question before it; ctrl+c leaves the
// box as it was.
func TestEscGoesBackFromTheName(t *testing.T) {
	v := &askPlugin{}
	m, c := named(t, v)
	k, _ := keyOf("esc")
	land(t, m, m.key(k))
	if m.confirm == nil || m.confirm.line != nil || m.confirm.question != "Save this secret?" {
		t.Fatalf("esc goes back to the save question, got %+v", m.confirm)
	}
	land(t, m, m.confirmKey("y"))
	k, _ = keyOf("ctrl+c")
	if cmd := m.key(k); cmd != nil || m.confirm != nil {
		t.Fatal("ctrl+c closes the question and asks nothing more")
	}
	if len(c.sending) != 0 || string(c.input) != "deploy with "+pastedKey+" please" || c.intercepting || len(v.stored) != 0 {
		t.Fatalf("ctrl+c leaves the box: sent %d, box %q", len(c.sending), string(c.input))
	}
}

// Each secret in a message gets its own two questions, in order.
func TestTwoSecretsAreNamedInTurn(t *testing.T) {
	v := &askPlugin{named: true}
	m, c := boxWithKey(t, v)
	gh := "ghp_" + strings.Repeat("Zq7Wx3Rt9", 4)
	c.input = []rune(pastedKey + " and " + gh)
	land(t, m, m.sendPane(c, false))
	for _, name := range []string{"OPENAI_API_KEY", "GITHUB_TOKEN"} {
		if m.confirm == nil || m.confirm.line != nil {
			t.Fatalf("%s: want the save question, got %+v", name, m.confirm)
		}
		land(t, m, m.confirmKey("y"))
		if m.confirm == nil || m.confirm.line == nil || string(m.confirm.line.buf) != name {
			t.Fatalf("%s: want its name asked for, got %+v", name, m.confirm)
		}
		pressKeys(m, "_", "A")
		land(t, m, enter(m))
	}
	sent := sentText(t, c)
	if strings.Contains(sent, pastedKey) || strings.Contains(sent, gh) ||
		!strings.HasPrefix(sent, "{{secret:OPENAI_API_KEY_A}} and {{secret:GITHUB_TOKEN_A}}") {
		t.Errorf("sent %q", sent)
	}
}

// A paste while the name is asked for goes in the line, as one line, and
// not in the box under it.
func TestPasteGoesInTheLine(t *testing.T) {
	m, c := named(t, &askPlugin{})
	box := string(c.input)
	pressKeys(m, "ctrl+u")
	m.Update(tea.PasteMsg{Content: "MY_KEY\n"})
	if got := string(m.confirm.line.buf); got != "MY_KEY" || string(c.input) != box {
		t.Fatalf("line %q, box %q", got, string(c.input))
	}
}

// A long line scrolls sideways, keeping the cursor in the box.
func TestLineKeepsTheCursorInView(t *testing.T) {
	l := &confirmLine{buf: []rune(strings.Repeat("A", 40) + "Z")}
	l.pos = len(l.buf)
	if got := stripAnsi(l.view(20)); !strings.HasSuffix(got, "Z▏") || len([]rune(got)) > 20 {
		t.Errorf("at the end: %q", got)
	}
	l.pos = 0
	if got := stripAnsi(l.view(20)); !strings.HasPrefix(got, "❯ ▏A") || strings.Contains(got, "Z") || len([]rune(got)) > 20 {
		t.Errorf("at the start: %q", got)
	}
}
