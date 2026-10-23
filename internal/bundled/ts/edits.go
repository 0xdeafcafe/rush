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
	"unicode/utf16"

	"github.com/0xdeafcafe/photon/jsonx"
)

type textEdit struct {
	Range   lspRange `json:"range"`
	NewText string   `json:"newText"`
}

// workspaceEdit is LSP's, in either of its shapes: changes by URI, or
// documentChanges, which may also create, rename or delete files.
type workspaceEdit struct {
	Changes         map[string][]textEdit `json:"changes"`
	DocumentChanges []jsontext.Value      `json:"documentChanges"`
}

// fileOp is one of documentChanges' file operations.
type fileOp struct {
	Kind   string `json:"kind"`
	URI    string `json:"uri"`
	OldURI string `json:"oldUri"`
	NewURI string `json:"newUri"`
}

// docEdit is one of documentChanges' text edits.
type docEdit struct {
	TextDocument struct {
		URI string `json:"uri"`
	} `json:"textDocument"`
	Edits []textEdit `json:"edits"`
}

// edits is a workspace edit's text edits by file, and its file renames,
// in order; refused when it reaches outside root or creates or deletes.
func (w workspaceEdit) edits(root string) (map[string][]textEdit, [][2]string, error) {
	by := map[string][]textEdit{}
	var renames [][2]string
	inside := func(uri string) (string, error) {
		p := pathOf(uri)
		if rel, err := filepath.Rel(root, p); err != nil || strings.HasPrefix(rel, "..") {
			return "", fmt.Errorf("the server's edit reaches %s, outside %s: not applied", p, root)
		}
		return p, nil
	}
	for uri, es := range w.Changes {
		p, err := inside(uri)
		if err != nil {
			return nil, nil, err
		}
		by[p] = append(by[p], es...)
	}
	for _, raw := range w.DocumentChanges {
		var op fileOp
		_ = jsonx.Unmarshal(raw, &op)
		switch op.Kind {
		case "":
			var d docEdit
			if err := jsonx.Unmarshal(raw, &d); err != nil {
				return nil, nil, err
			}
			p, err := inside(d.TextDocument.URI)
			if err != nil {
				return nil, nil, err
			}
			by[p] = append(by[p], d.Edits...)
		case "rename":
			from, err := inside(op.OldURI)
			if err != nil {
				return nil, nil, err
			}
			to, err := inside(op.NewURI)
			if err != nil {
				return nil, nil, err
			}
			renames = append(renames, [2]string{from, to})
		default:
			return nil, nil, fmt.Errorf("the server's edit would %s %s: not applied", op.Kind, pathOf(op.URI))
		}
	}
	return by, renames, nil
}

// apply writes edits into text: LSP positions are 0-based lines and
// UTF-16 columns, and each edit is against the text before any of them.
func apply(text string, edits []textEdit) (string, error) {
	lines := strings.SplitAfter(text, "\n")
	offset := func(p Position) (int, error) {
		if p.Line > len(lines) || p.Line == len(lines) && p.Character > 0 {
			return 0, fmt.Errorf("an edit at line %d is past the end", p.Line+1)
		}
		at := 0
		for _, l := range lines[:p.Line] {
			at += len(l)
		}
		if p.Line == len(lines) {
			return at, nil
		}
		units := 0
		for i, r := range lines[p.Line] {
			if units >= p.Character {
				return at + i, nil
			}
			units += len(utf16.Encode([]rune{r}))
		}
		return at + len(strings.TrimRight(lines[p.Line], "\n")), nil
	}
	type span struct {
		from, to int
		text     string
	}
	spans := make([]span, 0, len(edits))
	for _, e := range edits {
		from, err := offset(e.Range.Start)
		if err != nil {
			return "", err
		}
		to, err := offset(e.Range.End)
		if err != nil {
			return "", err
		}
		spans = append(spans, span{from, to, e.NewText})
	}
	sort.SliceStable(spans, func(i, j int) bool { return spans[i].from > spans[j].from })
	for i := 1; i < len(spans); i++ {
		if spans[i].to > spans[i-1].from {
			return "", errors.New("the server's edits overlap: not applied")
		}
	}
	out := text
	for _, s := range spans {
		out = out[:s.from] + s.text + out[s.to:]
	}
	return out, nil
}

// applyEdit writes a workspace edit to disk, all of it or none: every
// file's new text is worked out before any is written. It is the files
// changed, from cwd.
func (s *Server) applyEdit(w workspaceEdit, cwd string) ([]string, error) {
	by, renames, err := w.edits(s.Root)
	if err != nil {
		return nil, err
	}
	news := map[string]string{}
	for p, es := range by {
		b, err := os.ReadFile(p)
		if err != nil {
			return nil, err
		}
		out, err := apply(string(b), es)
		if err != nil {
			return nil, fmt.Errorf("%s: %w", p, err)
		}
		news[p] = out
	}
	var changed []string
	for p, text := range news {
		if err := os.WriteFile(p, []byte(text), 0o644); err != nil {
			return changed, err
		}
		changed = append(changed, rel(p, cwd))
	}
	for _, r := range renames {
		if err := moveFile(r[0], r[1]); err != nil {
			return changed, err
		}
		changed = append(changed, rel(r[0], cwd)+" → "+rel(r[1], cwd))
	}
	var events []map[string]any
	for p := range news {
		events = append(events, map[string]any{"uri": fileURI(p), "type": 2})
	}
	if len(events) > 0 {
		s.notify("workspace/didChangeWatchedFiles", map[string]any{"changes": events})
	}
	sort.Strings(changed)
	return changed, nil
}

func moveFile(from, to string) error {
	if _, err := os.Stat(to); err == nil {
		return fmt.Errorf("%s is there already", to)
	}
	if err := os.MkdirAll(filepath.Dir(to), 0o755); err != nil {
		return err
	}
	return os.Rename(from, to)
}

func rel(p, cwd string) string {
	if r, err := filepath.Rel(cwd, p); err == nil && !strings.HasPrefix(r, "..") {
		return r
	}
	return p
}

// rename renames the symbol at pos everywhere it's used, and writes it.
func rename(ctx context.Context, s *Server, uri string, pos Position, a Ask, cwd string) (string, error) {
	if a.NewName == "" {
		return "", errors.New("give new_name")
	}
	var w workspaceEdit
	if err := s.Call(ctx, "textDocument/rename", docParams(uri, pos, map[string]any{"newName": a.NewName}), &w); err != nil {
		return "", err
	}
	changed, err := s.applyEdit(w, cwd)
	if err != nil {
		return "", err
	}
	if len(changed) == 0 {
		return "nothing to rename there", nil
	}
	return fmt.Sprintf("renamed to %s in %d files:\n%s", a.NewName, len(changed), strings.Join(changed, "\n")), nil
}

// renameFile moves a file or folder and rewrites the imports that name
// it, as the server says to.
func renameFile(ctx context.Context, s *Server, uri string, _ Position, a Ask, cwd string) (string, error) {
	if a.To == "" {
		return "", errors.New("give to: the new path")
	}
	from := pathOf(uri)
	to := a.To
	if !filepath.IsAbs(to) {
		to = filepath.Join(cwd, to)
	}
	if r, err := filepath.Rel(s.Root, to); err != nil || strings.HasPrefix(r, "..") {
		return "", fmt.Errorf("%s is outside %s", to, s.Root)
	}
	files := []map[string]any{{"oldUri": fileURI(from), "newUri": fileURI(to)}}
	var w workspaceEdit
	if err := s.Call(ctx, "workspace/willRenameFiles", map[string]any{"files": files}, &w); err != nil {
		return "", err
	}
	changed, err := s.applyEdit(w, cwd)
	if err != nil {
		return "", err
	}
	if err := moveFile(from, to); err != nil {
		return "", fmt.Errorf("imports rewritten in %d files, but the move failed: %w", len(changed), err)
	}
	s.forget(uri)
	s.notify("workspace/didRenameFiles", map[string]any{"files": files})
	out := fmt.Sprintf("moved %s → %s", rel(from, cwd), rel(to, cwd))
	if len(changed) > 0 {
		out += fmt.Sprintf("; imports rewritten in %d files:\n%s", len(changed), strings.Join(changed, "\n"))
	}
	return out, nil
}

// forget closes uri if it was open: it's gone from where it was.
func (s *Server) forget(uri string) {
	s.mu.Lock()
	_, ok := s.opened[uri]
	delete(s.opened, uri)
	s.mu.Unlock()
	if ok {
		s.notify("textDocument/didClose", map[string]any{"textDocument": map[string]any{"uri": uri}})
	}
}
