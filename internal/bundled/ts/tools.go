package ts

import (
	"context"
	"encoding/json/jsontext"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"
	"unicode/utf16"
	"unicode/utf8"

	"github.com/0xdeafcafe/photon/jsonx"
)

// An Ask is one tool call's arguments: where, as file and 1-based line
// with a column or a symbol on that line, or a symbol alone to search for.
type Ask struct {
	File      string `json:"file"`
	Line      int    `json:"line"`
	Character int    `json:"character"`
	Symbol    string `json:"symbol"`
	Query     string `json:"query"`
	Direction string `json:"direction"`
}

// An Op is one tool: its name, what it's for, and what it does.
type Op struct {
	Name, Description string
	Needs             string // "position", "file" or "query"
	Do                func(ctx context.Context, s *Server, uri string, pos Position, a Ask, cwd string) (string, error)
}

// A Position is LSP's: a 0-based line and UTF-16 column.
type Position struct {
	Line      int `json:"line"`
	Character int `json:"character"`
}

type lspRange struct {
	Start Position `json:"start"`
	End   Position `json:"end"`
}

// location is a Location or a LocationLink, as servers answer either.
type location struct {
	URI                  string    `json:"uri"`
	Range                *lspRange `json:"range"`
	TargetURI            string    `json:"targetUri"`
	TargetSelectionRange *lspRange `json:"targetSelectionRange"`
}

func (l location) at() (string, Position) {
	if l.TargetURI != "" && l.TargetSelectionRange != nil {
		return l.TargetURI, l.TargetSelectionRange.Start
	}
	if l.Range != nil {
		return l.URI, l.Range.Start
	}
	return l.URI, Position{}
}

// Ops are the tools, in the order they're listed.
var Ops = []Op{
	{Name: "definition", Needs: "position", Description: "Where the symbol at a position is declared.", Do: locations("textDocument/definition", nil)},
	{Name: "references", Needs: "position", Description: "Every use of the symbol at a position, across the checkout, its declaration left out.",
		Do: locations("textDocument/references", map[string]any{"context": map[string]any{"includeDeclaration": false}})},
	{Name: "implementation", Needs: "position", Description: "What implements the interface or abstract member at a position.", Do: locations("textDocument/implementation", nil)},
	{Name: "hover", Needs: "position", Description: "The type and docs of the symbol at a position.", Do: hover},
	{Name: "callers", Needs: "position", Description: "The functions that call the function at a position.", Do: calls("callHierarchy/incomingCalls", "from")},
	{Name: "callees", Needs: "position", Description: "The functions the function at a position calls.", Do: calls("callHierarchy/outgoingCalls", "to")},
	{Name: "outline", Needs: "file", Description: "A file's declarations, nested, with their lines: read only the range you need.", Do: outline},
	{Name: "diagnostics", Needs: "file", Description: "A file's type errors and warnings, as the compiler sees them now.", Do: diagnostics},
	{Name: "find_symbol", Needs: "query", Description: "Declarations whose name matches query, across the checkout. In a monorepo this loads every " +
		"project: prefer a position-taking tool with file and line when you know where to look.", Do: findSymbol},
}

// OpNamed is the op called name, or nil.
func OpNamed(name string) *Op {
	for i := range Ops {
		if Ops[i].Name == name {
			return &Ops[i]
		}
	}
	return nil
}

// callTimeout is how long one call may take; a cold monorepo takes a while.
const callTimeout = 2 * time.Minute

// Do does op for a, asked from cwd.
func Do(ctx context.Context, p *Pool, op *Op, a Ask, cwd string) (string, error) {
	ctx, cancel := context.WithTimeout(ctx, callTimeout)
	defer cancel()
	file := a.File
	if file != "" && !filepath.IsAbs(file) {
		file = filepath.Join(cwd, file)
	}
	anchor := file
	if anchor == "" {
		anchor = cwd
	}
	s, err := p.For(ctx, anchor)
	if err != nil {
		return "", err
	}
	if op.Needs == "query" {
		if a.Query == "" {
			a.Query = a.Symbol
		}
		if a.Query == "" {
			return "", errors.New("give query: the name, or part of it, to look for")
		}
		return op.Do(ctx, s, "", Position{}, a, cwd)
	}
	if file == "" {
		return "", errors.New("give file, and for this tool a line with a character or a symbol on it")
	}
	uri, err := s.Open(file)
	if err != nil {
		return "", err
	}
	var pos Position
	if op.Needs == "position" {
		if pos, err = position(file, a); err != nil {
			return "", err
		}
	}
	return op.Do(ctx, s, uri, pos, a, cwd)
}

// position is where a points in file: its column, or where its symbol
// first stands whole on its line.
func position(file string, a Ask) (Position, error) {
	if a.Line < 1 {
		return Position{}, errors.New("give line, from 1, with character or symbol")
	}
	b, err := os.ReadFile(file)
	if err != nil {
		return Position{}, err
	}
	lines := strings.Split(string(b), "\n")
	if a.Line > len(lines) {
		return Position{}, fmt.Errorf("%s has %d lines, not %d", file, len(lines), a.Line)
	}
	text := lines[a.Line-1]
	switch {
	case a.Character > 0:
		return Position{Line: a.Line - 1, Character: utf16Len(prefixRunes(text, a.Character-1))}, nil
	case a.Symbol != "":
		i := wordIndex(text, a.Symbol)
		if i < 0 {
			return Position{}, fmt.Errorf("no %q on line %d of %s: %s", a.Symbol, a.Line, file, strings.TrimSpace(text))
		}
		return Position{Line: a.Line - 1, Character: utf16Len(text[:i])}, nil
	}
	return Position{}, errors.New("give character (from 1) or symbol, to say where on the line")
}

// wordIndex is the byte offset of word's first whole occurrence in s; -1
// for none.
func wordIndex(s, word string) int {
	isWord := func(r rune) bool {
		return r == '_' || r == '$' || r >= '0' && r <= '9' || r >= 'a' && r <= 'z' || r >= 'A' && r <= 'Z'
	}
	for off := 0; ; {
		i := strings.Index(s[off:], word)
		if i < 0 {
			return -1
		}
		i += off
		before, _ := utf8.DecodeLastRuneInString(s[:i])
		after, _ := utf8.DecodeRuneInString(s[i+len(word):])
		if (i == 0 || !isWord(before)) && (i+len(word) == len(s) || !isWord(after)) {
			return i
		}
		off = i + 1
	}
}

func prefixRunes(s string, n int) string {
	for i := range s {
		if n == 0 {
			return s[:i]
		}
		n--
	}
	return s
}

func utf16Len(s string) int { return len(utf16.Encode([]rune(s))) }

func docParams(uri string, pos Position, extra map[string]any) map[string]any {
	m := map[string]any{"textDocument": map[string]any{"uri": uri}, "position": pos}
	for k, v := range extra {
		m[k] = v
	}
	return m
}

func locations(method string, extra map[string]any) func(context.Context, *Server, string, Position, Ask, string) (string, error) {
	return func(ctx context.Context, s *Server, uri string, pos Position, _ Ask, cwd string) (string, error) {
		var res jsontext.Value
		if err := s.Call(ctx, method, docParams(uri, pos, extra), &res); err != nil {
			return "", err
		}
		locs, err := decodeLocations(res)
		if err != nil {
			return "", err
		}
		if len(locs) == 0 {
			return "none", nil
		}
		out := make([]string, 0, len(locs))
		for _, l := range locs {
			u, p := l.at()
			out = append(out, where(u, p, cwd))
		}
		sort.Strings(out)
		return limit(out, 200), nil
	}
}

func decodeLocations(res jsontext.Value) ([]location, error) {
	switch strings.TrimSpace(string(res))[:min(1, len(strings.TrimSpace(string(res))))] {
	case "", "n":
		return nil, nil
	case "{":
		var l location
		err := jsonx.Unmarshal(res, &l)
		return []location{l}, err
	}
	var ls []location
	err := jsonx.Unmarshal(res, &ls)
	return ls, err
}

// where is "path:line:col: the line", path from cwd when it's under it.
func where(uri string, p Position, cwd string) string {
	path := pathOf(uri)
	text := ""
	if b, err := os.ReadFile(path); err == nil {
		if lines := strings.Split(string(b), "\n"); p.Line < len(lines) {
			text = strings.TrimSpace(lines[p.Line])
			if len(text) > 160 {
				text = text[:160] + "…"
			}
		}
	}
	if rel, err := filepath.Rel(cwd, path); err == nil && !strings.HasPrefix(rel, "..") {
		path = rel
	}
	return fmt.Sprintf("%s:%d:%d: %s", path, p.Line+1, p.Character+1, text)
}

func limit(lines []string, n int) string {
	if len(lines) <= n {
		return strings.Join(lines, "\n")
	}
	return strings.Join(lines[:n], "\n") + fmt.Sprintf("\n… and %d more", len(lines)-n)
}

func hover(ctx context.Context, s *Server, uri string, pos Position, _ Ask, _ string) (string, error) {
	var res struct {
		Contents jsontext.Value `json:"contents"`
	}
	if err := s.Call(ctx, "textDocument/hover", docParams(uri, pos, nil), &res); err != nil {
		return "", err
	}
	if t := markup(res.Contents); t != "" {
		return t, nil
	}
	return "nothing to say about that position", nil
}

// markup is hover contents as text: MarkupContent, a MarkedString, or a
// list of them.
func markup(v jsontext.Value) string {
	var mc struct {
		Value string `json:"value"`
	}
	if jsonx.Unmarshal(v, &mc) == nil && mc.Value != "" {
		return strings.TrimSpace(mc.Value)
	}
	var str string
	if jsonx.Unmarshal(v, &str) == nil {
		return strings.TrimSpace(str)
	}
	var list []jsontext.Value
	if jsonx.Unmarshal(v, &list) == nil {
		var out []string
		for _, x := range list {
			if t := markup(x); t != "" {
				out = append(out, t)
			}
		}
		return strings.Join(out, "\n\n")
	}
	return ""
}

type hierarchyItem struct {
	Name           string   `json:"name"`
	Kind           int      `json:"kind"`
	URI            string   `json:"uri"`
	SelectionRange lspRange `json:"selectionRange"`
}

func calls(method, side string) func(context.Context, *Server, string, Position, Ask, string) (string, error) {
	return func(ctx context.Context, s *Server, uri string, pos Position, _ Ask, cwd string) (string, error) {
		var items []jsontext.Value
		if err := s.Call(ctx, "textDocument/prepareCallHierarchy", docParams(uri, pos, nil), &items); err != nil {
			return "", err
		}
		if len(items) == 0 {
			return "no function at that position", nil
		}
		var res []map[string]jsontext.Value
		if err := s.Call(ctx, method, map[string]any{"item": items[0]}, &res); err != nil {
			return "", err
		}
		var out []string
		for _, c := range res {
			var it hierarchyItem
			if jsonx.Unmarshal(c[side], &it) != nil {
				continue
			}
			out = append(out, fmt.Sprintf("%s %s  %s", kindName(it.Kind), it.Name, where(it.URI, it.SelectionRange.Start, cwd)))
		}
		if len(out) == 0 {
			return "none", nil
		}
		sort.Strings(out)
		return limit(out, 200), nil
	}
}

type docSymbol struct {
	Name           string      `json:"name"`
	Kind           int         `json:"kind"`
	Range          lspRange    `json:"range"`
	SelectionRange *lspRange   `json:"selectionRange"`
	Children       []docSymbol `json:"children"`
	Location       *location   `json:"location"`
	ContainerName  string      `json:"containerName"`
}

func outline(ctx context.Context, s *Server, uri string, _ Position, _ Ask, _ string) (string, error) {
	var syms []docSymbol
	if err := s.Call(ctx, "textDocument/documentSymbol", map[string]any{"textDocument": map[string]any{"uri": uri}}, &syms); err != nil {
		return "", err
	}
	var out []string
	var walk func([]docSymbol, int)
	walk = func(ss []docSymbol, depth int) {
		for _, d := range ss {
			r := d.Range
			if d.Location != nil && d.Location.Range != nil {
				r = *d.Location.Range
			}
			out = append(out, fmt.Sprintf("%s%s %s  %d-%d", strings.Repeat("  ", depth), kindName(d.Kind), d.Name, r.Start.Line+1, r.End.Line+1))
			walk(d.Children, depth+1)
		}
	}
	walk(syms, 0)
	if len(out) == 0 {
		return "no declarations", nil
	}
	return limit(out, 400), nil
}

func diagnostics(ctx context.Context, s *Server, uri string, _ Position, _ Ask, cwd string) (string, error) {
	raw, err := s.Diagnostics(ctx, uri, 15*time.Second)
	if err != nil {
		return "", err
	}
	var ds []struct {
		Range    lspRange       `json:"range"`
		Severity int            `json:"severity"`
		Message  string         `json:"message"`
		Code     jsontext.Value `json:"code"`
	}
	if len(raw) > 0 {
		if err := jsonx.Unmarshal(raw, &ds); err != nil {
			return "", err
		}
	}
	if len(ds) == 0 {
		return "no problems", nil
	}
	sev := map[int]string{1: "error", 2: "warning", 3: "info", 4: "hint"}
	path := pathOf(uri)
	if rel, err := filepath.Rel(cwd, path); err == nil && !strings.HasPrefix(rel, "..") {
		path = rel
	}
	out := make([]string, 0, len(ds))
	for _, d := range ds {
		code := strings.Trim(string(d.Code), `"`)
		if code != "" {
			code = " TS" + strings.TrimPrefix(code, "TS")
		}
		out = append(out, fmt.Sprintf("%s:%d:%d: %s%s: %s", path, d.Range.Start.Line+1, d.Range.Start.Character+1,
			sev[d.Severity], code, strings.ReplaceAll(d.Message, "\n", " ")))
	}
	return limit(out, 200), nil
}

func findSymbol(ctx context.Context, s *Server, _ string, _ Position, a Ask, cwd string) (string, error) {
	var syms []docSymbol
	if err := s.Call(ctx, "workspace/symbol", map[string]any{"query": a.Query}, &syms); err != nil {
		return "", err
	}
	var out []string
	for _, d := range syms {
		if d.Location == nil {
			continue
		}
		u, p := d.Location.at()
		if !strings.HasPrefix(pathOf(u), s.Root) || strings.Contains(pathOf(u), string(filepath.Separator)+"node_modules"+string(filepath.Separator)) {
			continue
		}
		out = append(out, fmt.Sprintf("%s %s  %s", kindName(d.Kind), d.Name, where(u, p, cwd)))
	}
	if len(out) == 0 {
		return "none", nil
	}
	sort.Strings(out)
	return limit(out, 100), nil
}

// kindName is LSP's SymbolKind in a word.
func kindName(k int) string {
	names := []string{"", "file", "module", "namespace", "package", "class", "method", "property", "field", "constructor",
		"enum", "interface", "function", "variable", "constant", "string", "number", "boolean", "array", "object",
		"key", "null", "enum-member", "struct", "event", "operator", "type-parameter"}
	if k > 0 && k < len(names) {
		return names[k]
	}
	return "symbol"
}
