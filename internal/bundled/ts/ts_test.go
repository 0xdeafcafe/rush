package ts

import (
	"context"
	"encoding/json/jsontext"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestWordIndex(t *testing.T) {
	for _, c := range []struct {
		s, w string
		want int
	}{
		{"const userName = user;", "user", 17},
		{"user.id", "user", 0},
		{"$user + user", "user", 8},
		{"users", "user", -1},
	} {
		if got := wordIndex(c.s, c.w); got != c.want {
			t.Errorf("wordIndex(%q, %q) = %d, want %d", c.s, c.w, got, c.want)
		}
	}
}

// LSP columns count UTF-16 units: an emoji before a name is two.
func TestPositionCountsUTF16(t *testing.T) {
	f := filepath.Join(t.TempDir(), "a.ts")
	if err := os.WriteFile(f, []byte("const a = 1;\nconst s = \"😀\"; foo();\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	p, err := position(f, Ask{Line: 2, Symbol: "foo"})
	if err != nil || p.Line != 1 || p.Character != 16 {
		t.Fatalf("position = %+v, %v; want line 1 character 16", p, err)
	}
	if _, err := position(f, Ask{Line: 2, Symbol: "bar"}); err == nil {
		t.Fatal("found a name that isn't on the line")
	}
	if p, _ := position(f, Ask{Line: 1, Character: 7}); p.Character != 6 {
		t.Fatalf("character 7 from 1 = %d, want 6", p.Character)
	}
}

func TestDecodeLocations(t *testing.T) {
	one, _ := decodeLocations(jsontext.Value(`{"uri":"file:///a.ts","range":{"start":{"line":2,"character":3},"end":{"line":2,"character":4}}}`))
	links, _ := decodeLocations(jsontext.Value(`[{"targetUri":"file:///b.ts","targetSelectionRange":{"start":{"line":5,"character":1},"end":{"line":5,"character":2}}}]`))
	none, _ := decodeLocations(jsontext.Value(`null`))
	if u, p := one[0].at(); u != "file:///a.ts" || p.Line != 2 {
		t.Errorf("Location = %s %+v", u, p)
	}
	if u, p := links[0].at(); u != "file:///b.ts" || p.Line != 5 {
		t.Errorf("LocationLink = %s %+v", u, p)
	}
	if len(none) != 0 {
		t.Errorf("null = %v", none)
	}
}

func TestMarkup(t *testing.T) {
	for in, want := range map[string]string{
		`{"kind":"markdown","value":"**x**"}`: "**x**",
		`"plain"`:                             "plain",
		`["a",{"language":"ts","value":"b"}]`: "a\n\nb",
	} {
		if got := markup(jsontext.Value(in)); got != want {
			t.Errorf("markup(%s) = %q, want %q", in, got, want)
		}
	}
}

// One server per checkout: the root is the .git's folder, not the
// nearest tsconfig's, so a monorepo's packages share one.
func TestWorkspaceRoot(t *testing.T) {
	root := t.TempDir()
	pkg := filepath.Join(root, "packages", "a", "src")
	_ = os.MkdirAll(pkg, 0o700)
	_ = os.WriteFile(filepath.Join(root, ".git"), []byte("gitdir: elsewhere\n"), 0o600)
	_ = os.WriteFile(filepath.Join(root, "packages", "a", "tsconfig.json"), []byte("{}"), 0o600)
	f := filepath.Join(pkg, "x.ts")
	_ = os.WriteFile(f, nil, 0o600)
	if got := WorkspaceRoot(f); got != root {
		t.Fatalf("WorkspaceRoot = %s, want %s", got, root)
	}
	_ = os.Remove(filepath.Join(root, ".git"))
	if got := WorkspaceRoot(f); got != filepath.Join(root, "packages", "a") {
		t.Fatalf("without a .git, WorkspaceRoot = %s, want the tsconfig's folder", got)
	}
}

// The platform binary, never the package's Node shim, wherever pnpm or
// npm put it.
func TestResolveFindsThePlatformBinary(t *testing.T) {
	root := t.TempDir()
	store := filepath.Join(root, "node_modules", ".pnpm", "x", "node_modules", "@maschwenk")
	bin := filepath.Join(store, "tsrs-"+platform(), "tsrs")
	_ = os.MkdirAll(filepath.Dir(bin), 0o700)
	_ = os.MkdirAll(filepath.Join(store, "tsrs", "bin"), 0o700)
	_ = os.WriteFile(bin, []byte("#!/bin/sh\n"), 0o700)
	_ = os.MkdirAll(filepath.Join(root, "node_modules", "@maschwenk"), 0o700)
	if err := os.Symlink(filepath.Join(store, "tsrs"), filepath.Join(root, "node_modules", "@maschwenk", "tsrs")); err != nil {
		t.Fatal(err)
	}
	e, err := Resolve(root, "auto")
	if err != nil || e.Name != "tsrs" {
		t.Fatalf("Resolve = %+v, %v", e, err)
	}
	if real, _ := filepath.EvalSymlinks(bin); e.Bin != real && e.Bin != bin {
		t.Fatalf("Resolve bin = %s, want %s", e.Bin, bin)
	}
}

// A server past its cap is stopped, the call told why, and the next call
// gets a fresh one. Skipped where no server is installed.
func TestPoolCapsAServer(t *testing.T) {
	root := t.TempDir()
	_ = os.WriteFile(filepath.Join(root, "tsconfig.json"), []byte("{}"), 0o600)
	f := filepath.Join(root, "a.ts")
	_ = os.WriteFile(f, []byte("export const a = 1;\n"), 0o600)
	if _, err := Resolve(root, "auto"); err != nil {
		t.Skip(err)
	}
	t.Setenv("RUSH_GATE_HELD", "test")
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	p := NewPool(ctx, Options{Engine: "auto", Cap: 1 << 20})
	defer p.CloseAll()
	s, err := p.For(ctx, f)
	if err != nil {
		t.Fatal(err)
	}
	select {
	case <-s.Done():
	case <-time.After(20 * time.Second):
		t.Fatal("not stopped past its cap")
	}
	if why := s.Why(); why == nil || !strings.Contains(why.Error(), "past its") {
		t.Fatalf("why = %v", why)
	}
	p.SetOptions(Options{Engine: "auto"})
	again, err := p.For(ctx, f)
	if err != nil || again == s {
		t.Fatalf("no fresh server: %v", err)
	}
	if out, err := Do(ctx, p, OpNamed("hover"), Ask{File: f, Line: 1, Symbol: "a"}, root); err != nil || !strings.Contains(out, "a") {
		t.Fatalf("hover = %q, %v", out, err)
	}
}
