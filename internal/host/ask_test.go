package host

import (
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
)

// fakeAsker answers control requests: a side question, and what fills the
// context, which the host asks after every turn.
const fakeAsker = `#!/bin/sh
sid=SID; prev=; for a in "$@"; do case "$prev" in --session-id|--resume) sid=$a ;; esac; prev=$a; done
echo '{"type":"system","subtype":"init","session_id":"'"$sid"'","model":"claude-haiku-4-5","permissionMode":"default","tools":["Bash"]}'
while read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"request_id":"\([^"]*\)".*/\1/p')
  case "$line" in
  *'"subtype":"side_question"'*)
    echo '{"type":"control_response","response":{"subtype":"success","request_id":"'"$id"'","response":{"response":"four","synthetic":false}}}'
    ;;
  *'"subtype":"get_context_usage"'*)
    echo '{"type":"control_response","response":{"subtype":"success","request_id":"'"$id"'","response":{"totalTokens":1200,"maxTokens":200000,"categories":[{"name":"Messages","tokens":1200,"kind":"used"}]}}}'
    ;;
  *'"type":"user"'*)
    echo '{"type":"assistant","message":{"id":"m1","role":"assistant","content":[{"type":"text","text":"hi"}]}}'
    echo '{"type":"result","subtype":"success","result":"hi","total_cost_usd":0.01}'
    ;;
  esac
done
`

func TestAskAndContext(t *testing.T) {
	bin := setup(t)
	if err := os.WriteFile(bin, []byte(fakeAsker), 0o755); err != nil {
		t.Fatal(err)
	}
	cfg, err := Spawn(Config{Cwd: filepath.Dir(bin), Prompt: "hello", Binary: bin, IdleStop: Duration(time.Minute)})
	if err != nil {
		t.Fatal(err)
	}
	c, err := Dial(cfg.ID)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = c.Stop(); c.Close() }()

	// After the turn, the host counts the context on its own.
	ctx := next(t, c, func(ev any) bool { _, ok := ev.(Context); return ok }).(Context)
	if ctx.Usage.Total != 1200 || ctx.Usage.Max != 200000 || ctx.Usage.At.IsZero() {
		t.Fatalf("context: %+v", ctx.Usage)
	}
	// A control request passed through comes back under the client's id.
	if err := c.Ask("ui-1", map[string]any{"subtype": "side_question", "question": "2+2?", "history": []any{}}); err != nil {
		t.Fatal(err)
	}
	r := next(t, c, func(ev any) bool { r, ok := ev.(Reply); return ok && r.ID == "ui-1" }).(Reply)
	var a struct{ Response string }
	if jsonx.Unmarshal(r.Body, &a); a.Response != "four" || r.Error != "" {
		t.Fatalf("reply: %+v %s", r, r.Body)
	}
	// A client that connects later is told the last count.
	c2, err := Dial(cfg.ID)
	if err != nil {
		t.Fatal(err)
	}
	defer c2.Close()
	next(t, c2, func(ev any) bool { x, ok := ev.(Context); return ok && x.Usage.Total == 1200 })
}
