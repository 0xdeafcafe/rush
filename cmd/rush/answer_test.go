package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// askingClaude asks a question through can_use_tool on every message, and
// logs the control responses it gets back.
const askingClaude = `#!/bin/sh
sid=SID; prev=; for a in "$@"; do case "$prev" in --session-id|--resume) sid=$a ;; esac; prev=$a; done
echo '{"type":"system","subtype":"init","session_id":"'"$sid"'","model":"claude-haiku-4-5","permissionMode":"default","tools":[]}'
n=0
while read -r line; do
  case "$line" in
  *'"type":"control_response"'*)
    printf '%s\n' "$line" >> "$(dirname "$0")/answers.log"
    ;;
  *'"type":"user"'*)
    n=$((n+1))
    echo '{"type":"control_request","request_id":"r'$n'","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[{"question":"Tea or coffee?","header":"Drink","multiSelect":false,"options":[{"label":"Tea","description":""},{"label":"Coffee","description":""}]}]},"tool_use_id":"t'$n'"}}'
    ;;
  esac
done
`

func answersLog(t *testing.T, dir string, n int) []string {
	t.Helper()
	path := filepath.Join(dir, "answers.log")
	var lines []string
	waitFor(t, "the answer", func() bool {
		body, _ := os.ReadFile(path)
		lines = strings.Split(strings.TrimSpace(string(body)), "\n")
		return len(body) > 0 && len(lines) >= n
	})
	return lines
}

func TestAnswerSettlesTheQuestion(t *testing.T) {
	bin := setup(t)
	if err := os.WriteFile(bin, []byte(askingClaude), 0o755); err != nil {
		t.Fatal(err)
	}
	dir := filepath.Dir(bin)
	startJSON(t, "--cwd", dir, "--session-id", sid, "--binary", bin, "--prompt-file", writeFile(t, dir, "p.txt", "ask me"))
	waitFor(t, "the question", func() bool {
		out, _ := run(t, "", "info", "11111111", "--json")
		return strings.Contains(out, "Tea or coffee?")
	})

	if out, code := run(t, "Coffee", "answer", "11111111", "--request", "nope"); code == 0 {
		t.Fatalf("an answer for another request went through: %s", out)
	}
	if out, code := run(t, "Coffee", "answer", "11111111", "--request", "t1"); code != 0 {
		t.Fatalf("answer: exit %d: %s", code, out)
	}
	got := answersLog(t, dir, 1)[0]
	for _, want := range []string{`"behavior":"allow"`, `"answers":{"Tea or coffee?":"Coffee"}`, `"request_id":"r1"`} {
		if !strings.Contains(got, want) {
			t.Fatalf("answer sent %s, want %s in it", got, want)
		}
	}
	if out, code := run(t, "Tea", "answer", "11111111"); code == 0 || !strings.Contains(out, "not waiting") {
		t.Fatalf("a second answer: exit %d: %s", code, out)
	}
}

func TestAnswerDenyDeclinesTheQuestion(t *testing.T) {
	bin := setup(t)
	if err := os.WriteFile(bin, []byte(askingClaude), 0o755); err != nil {
		t.Fatal(err)
	}
	dir := filepath.Dir(bin)
	startJSON(t, "--cwd", dir, "--session-id", sid, "--binary", bin, "--prompt-file", writeFile(t, dir, "p.txt", "ask me"))
	waitFor(t, "the question", func() bool {
		out, _ := run(t, "", "info", "11111111", "--json")
		return strings.Contains(out, "Tea or coffee?")
	})
	if out, code := run(t, "", "answer", "11111111", "--deny"); code != 0 {
		t.Fatalf("answer --deny: exit %d: %s", code, out)
	}
	got := answersLog(t, dir, 1)[0]
	if !strings.Contains(got, `"behavior":"deny"`) || !strings.Contains(got, "skipped the question") {
		t.Fatalf("deny sent %s", got)
	}
}
