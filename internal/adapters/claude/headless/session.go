package headless

import (
	"encoding/base64"
	"encoding/json/jsontext"
	"errors"
	"fmt"
	"io"
	"os/exec"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"syscall"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/adapters/claude/claude"
	"github.com/0xdeafcafe/rush/internal/pgguard"
)

// Options says which conversation to run and where.
type Options struct {
	Account claude.Account
	Dir     string
	// Resume continues a saved conversation by its session id. SessionID
	// instead names a new one; leave both empty to let Claude Code pick.
	Resume         string
	SessionID      string
	Model          string
	Effort         string // low, medium, high, xhigh, max; fixed for the process's life
	PermissionMode string // default, acceptEdits, plan, auto, ...
	Flags          []string
	// Binary is the claude executable; empty means "claude" on PATH.
	Binary string
	// Tap, when set, sees every output line before it is decoded.
	Tap func(line []byte)
	// Skip, when set, says which lines not to decode at all: they reach Tap
	// and nothing else. A host that only relays streamed deltas and tool
	// results needn't pay to take them apart.
	Skip func(line []byte) bool
	// Env is added to the account's environment.
	Env []string
}

func (o Options) args() []string {
	args := []string{"-p",
		"--input-format", "stream-json", "--output-format", "stream-json",
		"--include-partial-messages", "--verbose",
		// stdio routes every permission prompt to us as a can_use_tool request;
		// without it headless sessions deny them silently.
		"--permission-prompt-tool", "stdio",
	}
	if o.Resume != "" {
		args = append(args, "--resume", o.Resume)
	} else if o.SessionID != "" {
		args = append(args, "--session-id", o.SessionID)
	}
	if o.Model != "" {
		args = append(args, "--model", o.Model)
	}
	if o.Effort != "" {
		args = append(args, "--effort", o.Effort)
	}
	if o.PermissionMode != "" {
		args = append(args, "--permission-mode", o.PermissionMode)
	}
	return append(args, o.Flags...)
}

// Session is one running `claude -p`. Read its Events until the channel
// closes; Err then says why it ended.
type Session struct {
	Events <-chan Event

	cmd   *exec.Cmd
	stdin io.WriteCloser
	// Writes queue here and a goroutine feeds them to stdin, so a caller
	// holding a lock never blocks on Claude Code (which may itself be
	// blocked writing its output).
	wmu     sync.Mutex
	wbuf    [][]byte
	wsig    chan struct{}
	wclosed bool
	werr    error
	seq     atomic.Int64
	done    chan struct{}
	err     error
	stderr  tail
	guard   *pgguard.Guard
}

// RunsSession says whether args are a headless Claude Code on sessionID, as
// Start runs it.
func RunsSession(args []string, sessionID string) bool {
	if !slices.Contains(args, "-p") || !slices.Contains(args, "stream-json") {
		return false
	}
	for i := 0; i+1 < len(args); i++ {
		if (args[i] == "--resume" || args[i] == "--session-id") && args[i+1] == sessionID {
			return true
		}
	}
	return false
}

// Start launches Claude Code for o.
func Start(o Options) (*Session, error) {
	bin := o.Binary
	if bin == "" {
		bin = "claude"
	}
	cmd := exec.Command(bin, o.args()...)
	cmd.Dir = o.Dir
	// rush draws a question's option previews, which Claude Code only
	// offers a headless session when told the format.
	cmd.Env = append(append(o.Account.Env(), "CLAUDE_CODE_QUESTION_PREVIEW_FORMAT=markdown"), o.Env...)
	// Its own process group, so stopping the session takes its shells too.
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	stdin, err := cmd.StdinPipe()
	if err != nil {
		return nil, err
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return nil, err
	}
	events := make(chan Event, 256)
	s := &Session{Events: events, cmd: cmd, stdin: stdin, done: make(chan struct{}), wsig: make(chan struct{}, 1)}
	cmd.Stderr = &s.stderr
	if err := cmd.Start(); err != nil {
		return nil, err
	}
	s.guard = pgguard.Watch(cmd.Process.Pid)
	go s.read(stdout, events, o.Tap, o.Skip)
	go s.writer()
	return s, nil
}

// writer feeds queued writes to stdin until the session stops.
func (s *Session) writer() {
	for {
		select {
		case <-s.wsig:
		case <-s.done:
			return
		}
		s.wmu.Lock()
		batch, closed := s.wbuf, s.wclosed
		s.wbuf = nil
		s.wmu.Unlock()
		for _, b := range batch {
			if _, err := s.stdin.Write(b); err != nil {
				s.wmu.Lock()
				s.werr = err
				s.wmu.Unlock()
				break
			}
		}
		if closed {
			_ = s.stdin.Close()
			return
		}
	}
}

func (s *Session) signal() {
	select {
	case s.wsig <- struct{}{}:
	default:
	}
}

func (s *Session) read(r io.Reader, events chan<- Event, tap func([]byte), skip func([]byte) bool) {
	defer close(s.done)
	defer close(events)
	lines := jsonx.NewLineReader(r)
	for {
		line, ok := lines.Next()
		if !ok {
			break
		}
		if tap != nil {
			tap(line)
		}
		if skip != nil && skip(line) {
			continue
		}
		ev, err := Decode(line)
		if err != nil {
			continue
		}
		if o, ok := ev.(Other); ok && o.Type == "control_request" {
			// A request we never registered for; refuse it rather than
			// leave Claude Code waiting.
			var e envelope
			_ = jsonx.Unmarshal(o.Raw, &e)
			_ = s.reply(e.RequestID, nil, "not supported by rush")
			continue
		}
		events <- ev
	}
	if lines.Err() != nil {
		// Nobody reads its output any more, so it would block forever:
		// end it, and say why.
		_ = syscall.Kill(-s.cmd.Process.Pid, syscall.SIGKILL)
	}
	err := s.cmd.Wait()
	s.guard.Release()
	if lines.Err() != nil {
		err = fmt.Errorf("reading Claude Code's output: %w", lines.Err())
	}
	if err != nil {
		if t := s.stderr.String(); t != "" {
			err = fmt.Errorf("%w: %s", err, t)
		}
	}
	s.err = err
}

// PID is Claude Code's process id.
func (s *Session) PID() int { return s.cmd.Process.Pid }

// Done closes when the process has exited.
func (s *Session) Done() <-chan struct{} { return s.done }

// Err is why the session ended, once Done has closed.
func (s *Session) Err() error {
	<-s.done
	return s.err
}

func (s *Session) write(v any) error {
	b, err := jsonx.Marshal(v)
	if err != nil {
		return err
	}
	s.wmu.Lock()
	defer s.signal()
	defer s.wmu.Unlock()
	switch {
	case s.wclosed:
		return errors.New("session stopped")
	case s.werr != nil:
		return s.werr
	}
	s.wbuf = append(s.wbuf, append(b, '\n'))
	return nil
}

// Send queues a user message. Sent mid-turn, Claude Code picks it up at its
// next step, as typing into a busy session does.
func (s *Session) Send(text string) error { return s.SendWith(text, nil) }

// Image is a picture sent with a message.
type Image struct {
	MediaType string // image/png, image/jpeg, image/gif, image/webp
	Data      []byte
}

// SendWith sends a message with images attached.
func (s *Session) SendWith(text string, images []Image) error {
	var content any = text
	if len(images) > 0 {
		content = Content(text, images)
	}
	return s.write(map[string]any{
		"type":    "user",
		"message": map[string]any{"role": "user", "content": content},
	})
}

var markerRe = regexp.MustCompile(`\[Image #(\d+)\]`)

// Content is a message's content blocks. Where the text names an image by
// its marker, [Image #2] say, the text up to and including the marker comes
// first and that image right after it, so Claude sees each picture where it
// was put. Images the text doesn't name follow at the end; a text that names
// none keeps the images before the text.
func Content(text string, images []Image) []map[string]any {
	img := func(im Image) map[string]any {
		return map[string]any{"type": "image", "source": map[string]any{
			"type": "base64", "media_type": im.MediaType, "data": base64.StdEncoding.EncodeToString(im.Data)}}
	}
	txt := func(t string) map[string]any { return map[string]any{"type": "text", "text": t} }
	var blocks []map[string]any
	used := make([]bool, len(images))
	last := 0
	for _, m := range markerRe.FindAllStringSubmatchIndex(text, -1) {
		n, _ := strconv.Atoi(text[m[2]:m[3]])
		if n < 1 || n > len(images) || used[n-1] {
			continue
		}
		if chunk := text[last:m[1]]; strings.TrimSpace(chunk) != "" {
			blocks = append(blocks, txt(chunk))
		}
		blocks = append(blocks, img(images[n-1]))
		used[n-1], last = true, m[1]
	}
	if last == 0 {
		for _, im := range images {
			blocks = append(blocks, img(im))
		}
		if strings.TrimSpace(text) != "" {
			blocks = append(blocks, txt(text))
		}
		return blocks
	}
	if rest := text[last:]; strings.TrimSpace(rest) != "" {
		blocks = append(blocks, txt(rest))
	}
	for i, im := range images {
		if !used[i] {
			blocks = append(blocks, img(im))
		}
	}
	return blocks
}

// Allow lets a requested tool run. input is the tool input to use, usually
// the request's own; always adds the request's suggested rules so Claude
// Code stops asking for the same thing.
func (s *Session) Allow(req PermissionRequest, input jsontext.Value, always bool) error {
	if input == nil {
		input = req.Input
	}
	body := map[string]any{"behavior": "allow", "updatedInput": input, "toolUseID": req.ToolUseID}
	if always && len(req.Suggestions) > 0 {
		body["updatedPermissions"] = req.Suggestions
		body["decisionClassification"] = "user_permanent"
	} else {
		body["decisionClassification"] = "user_temporary"
	}
	return s.reply(req.ID, body, "")
}

// Deny refuses a requested tool. message is what Claude sees; interrupt also
// stops the turn.
func (s *Session) Deny(req PermissionRequest, message string, interrupt bool) error {
	if message == "" {
		message = "The user declined this tool call."
	}
	return s.reply(req.ID, map[string]any{
		"behavior": "deny", "message": message, "interrupt": interrupt,
		"toolUseID": req.ToolUseID, "decisionClassification": "user_reject",
	}, "")
}

func (s *Session) reply(id string, body any, errText string) error {
	resp := map[string]any{"subtype": "success", "request_id": id, "response": body}
	if errText != "" {
		resp = map[string]any{"subtype": "error", "request_id": id, "error": errText}
	}
	return s.write(map[string]any{"type": "control_response", "response": resp})
}

// control sends a request to Claude Code and returns its id; the answer
// arrives later as a ControlReply event.
func (s *Session) control(req map[string]any) (string, error) {
	id := fmt.Sprintf("rush-%d", s.seq.Add(1))
	return id, s.write(map[string]any{"type": "control_request", "request_id": id, "request": req})
}

// Initialize asks for the session's slash commands, models and account; the
// answer is a ControlReply for the returned id (read it with Commands). It
// also registers the MCP servers the host runs in-process, whose messages
// then arrive as MCPRequests; send it before the first message.
//
// It says rush stops background tasks one at a time (StopTask), so an
// interrupt stops only the turn and leaves running subagents be; without
// that, Claude Code kills them all with the turn.
func (s *Session) Initialize(servers ...string) (string, error) {
	req := map[string]any{"subtype": "initialize", "perTaskStopAffordance": true}
	if len(servers) > 0 {
		req["sdkMcpServers"] = servers
	}
	return s.control(req)
}

// ReplyMCP answers an MCPRequest with its server's JSON-RPC reply.
func (s *Session) ReplyMCP(id string, reply jsontext.Value) error {
	return s.reply(id, map[string]any{"mcp_response": reply}, "")
}

// Interrupt stops the current turn, as esc does.
func (s *Session) Interrupt() error {
	_, err := s.control(map[string]any{"subtype": "interrupt", "reason": "interrupt"})
	return err
}

// StopTask stops one background task, a subagent or a background shell,
// by its id; the turn and the other tasks carry on.
func (s *Session) StopTask(id string) error {
	_, err := s.control(map[string]any{"subtype": "stop_task", "task_id": id})
	return err
}

// Background moves a Bash command or subagent the turn is waiting on into
// the background, as ctrl+b does in Claude Code; the turn carries on
// without it. An empty toolUseID moves every one.
func (s *Session) Background(toolUseID string) error {
	req := map[string]any{"subtype": "background_tasks"}
	if toolUseID != "" {
		req["tool_use_id"] = toolUseID
	}
	_, err := s.control(req)
	return err
}

// SetPermissionMode switches between default, acceptEdits, plan, auto, ...
func (s *Session) SetPermissionMode(mode string) error {
	_, err := s.control(map[string]any{"subtype": "set_permission_mode", "mode": mode})
	return err
}

// SetModel switches model for the next turn; empty resets to the default.
func (s *Session) SetModel(model string) error {
	req := map[string]any{"subtype": "set_model"}
	if model != "" {
		req["model"] = model
	}
	_, err := s.control(req)
	return err
}

// Stop ends the session. Closing stdin lets Claude Code finish writing the
// transcript; it is killed if it has not gone after grace.
func (s *Session) Stop(grace time.Duration) error {
	s.wmu.Lock()
	s.wclosed = true
	s.wmu.Unlock()
	s.signal()
	select {
	case <-s.done:
	case <-time.After(grace):
		_ = syscall.Kill(-s.cmd.Process.Pid, syscall.SIGTERM)
		select {
		case <-s.done:
		case <-time.After(2 * time.Second):
			_ = syscall.Kill(-s.cmd.Process.Pid, syscall.SIGKILL)
			<-s.done
		}
	}
	var exit *exec.ExitError
	if errors.As(s.err, &exit) {
		return nil // stopped on purpose
	}
	return s.err
}

// tail keeps the last few KB of stderr for error messages.
type tail struct {
	mu  sync.Mutex
	buf []byte
}

func (t *tail) Write(p []byte) (int, error) {
	t.mu.Lock()
	defer t.mu.Unlock()
	t.buf = append(t.buf, p...)
	if n := len(t.buf); n > 4<<10 {
		t.buf = t.buf[n-4<<10:]
	}
	return len(p), nil
}

func (t *tail) String() string {
	t.mu.Lock()
	defer t.mu.Unlock()
	return strings.TrimSpace(string(t.buf))
}
