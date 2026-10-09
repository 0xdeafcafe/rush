package acp

import (
	"context"
	"crypto/md5"
	"errors"
	"fmt"
	"maps"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
)

// Kimi adds Kimi's configured models and on-disk sessions to ACP.
type Kimi struct{ Agent }

// kimiHome is where Kimi keeps its config and sign-in: Kimi Code's
// ~/.kimi-code (KIMI_CODE_HOME), else the older Python kimi-cli's ~/.kimi
// (KIMI_SHARE_DIR).
func kimiHome() string {
	for _, v := range []string{"KIMI_CODE_HOME", "KIMI_SHARE_DIR"} {
		if d := os.Getenv(v); d != "" {
			return d
		}
	}
	h, _ := os.UserHomeDir()
	if _, err := os.Stat(filepath.Join(h, ".kimi-code", "config.toml")); err == nil {
		return filepath.Join(h, ".kimi-code")
	}
	return filepath.Join(h, ".kimi")
}

// kimiEnv points either Kimi at dir.
func kimiEnv(env []string, dir string) []string {
	return append(append([]string(nil), env...), "KIMI_CODE_HOME="+dir, "KIMI_SHARE_DIR="+dir)
}
func (a Kimi) Profiles() []agent.Profile {
	if !agent.Installed(a.ID) {
		return nil
	}
	return []agent.Profile{{Kind: a.ID, Name: a.Title, Dir: kimiHome()}}
}
func (a Kimi) Program() (string, []string) {
	return a.Command, []string{".kimi-code/bin", ".local/bin"}
}
func (a Kimi) Start(ctx context.Context, o agent.StartOptions) (agent.Conn, error) {
	if o.Profile.Dir != "" {
		o.Env = kimiEnv(o.Env, o.Profile.Dir)
	}
	return a.Agent.Start(ctx, o)
}
func (a Kimi) Features() map[agent.Feature]agent.Support {
	out := maps.Clone(a.Agent.Features())
	out[agent.FeatureSignIn] = agent.Yes.With("native terminal login; no account swapping")
	out[agent.FeatureHistory] = agent.Yes.With("saved local conversations")
	out[agent.FeatureQuota] = agent.Yes.With("Kimi Code plans only, as its /usage reads them")
	out[agent.FeatureCompact] = agent.Yes.With("its own /compact")
	out[agent.FeatureBackground] = agent.Yes.With("background shells and subagents, ended as Kimi says")
	return out
}
func (a Kimi) Published() agent.Published {
	return agent.Published{Update: []string{"upgrade"}}
}
func (Kimi) Level() agent.Level { return agent.LevelTested }

// ponytail: Kimi's thinking levels and modes as its ACP session/new
// advertised them (2026-10); a running session's own list wins.
func (a Kimi) Choices() agent.Choices {
	return agent.Choices{
		Efforts: []agent.Choice{
			{ID: "low", Note: "thinks briefly; quick for simple tasks."},
			{ID: "high", Note: "thinks carefully."},
			{ID: "max", Note: "thinks as much as it can; Kimi's default."},
		},
		Modes: []agent.Choice{
			{ID: "default", Note: "asks before tools run."},
			{ID: "plan", Note: "reads and plans; runs no tools."},
			{ID: "auto", Note: "approves safe operations itself."},
			{ID: "yolo", Note: "approves everything; never asks."},
		},
	}
}
func (Kimi) ListModels(p agent.Profile) []agent.Choice {
	if p.Dir == "" {
		p.Dir = kimiHome()
	}
	cfg, err := readKimiConfig(p.Dir)
	if err != nil {
		return nil
	}
	var out []agent.Choice
	for id, m := range cfg.Models {
		note := m.Display
		if note == "" {
			note = m.Model
		}
		out = append(out, agent.Choice{ID: id, Note: note, Context: m.Context})
	}
	sort.Slice(out, func(i, j int) bool { return out[i].ID < out[j].ID })
	return out
}
func (Kimi) Live(agent.Profile) []agent.Session { return nil }
func (a Kimi) Past(p agent.Profile) []agent.Session {
	if p.Dir == "" {
		p.Dir = kimiHome()
	}
	out := append(a.codePast(p), a.legacyPast(p)...)
	sort.Slice(out, func(i, j int) bool { return out[i].UpdatedAt.After(out[j].UpdatedAt) })
	return out
}

func (a Kimi) legacyPast(p agent.Profile) []agent.Session {
	if p.Dir == "" {
		p.Dir = kimiHome()
	}
	var meta struct {
		Dirs []struct {
			Path string `json:"path"`
			Kaos string `json:"kaos"`
		} `json:"work_dirs"`
	}
	b, err := os.ReadFile(filepath.Join(p.Dir, "kimi.json"))
	if err != nil || jsonx.Unmarshal(b, &meta) != nil {
		return nil
	}
	var out []agent.Session
	for _, d := range meta.Dirs {
		if d.Kaos != "" && d.Kaos != "local" {
			continue
		}
		root := filepath.Join(p.Dir, "sessions", fmt.Sprintf("%x", md5.Sum([]byte(d.Path))))
		paths, _ := filepath.Glob(filepath.Join(root, "*", "context.jsonl"))
		for _, path := range paths {
			st, err := os.Stat(path)
			if err != nil || st.Size() == 0 {
				continue
			}
			id := filepath.Base(filepath.Dir(path))
			name := id
			var state struct {
				Title string `json:"custom_title"`
			}
			if b, err := os.ReadFile(filepath.Join(filepath.Dir(path), "state.json")); err == nil && jsonx.Unmarshal(b, &state) == nil && state.Title != "" {
				name = state.Title
			}
			out = append(out, agent.Session{Kind: a.ID, Profile: p, ID: id, Name: name, Cwd: d.Path, Transcript: path, State: "done", UpdatedAt: st.ModTime()})
		}
	}
	sort.Slice(out, func(i, j int) bool { return out[i].UpdatedAt.After(out[j].UpdatedAt) })
	return out
}
func (Kimi) History(s agent.Session, before time.Time) ([]event.Event, error) {
	if filepath.Base(s.Transcript) == "wire.jsonl" {
		evs, _, err := readKimiWireFile(s.Transcript, 0, before)
		return evs, err
	}
	// Legacy context records have no timestamps.
	return ReadMessagesBefore(s.Transcript, before)
}

var _ agent.ModelLister = Kimi{}
var _ agent.Discoverer = Kimi{}
var _ agent.HistoryReader = Kimi{}

// CheckKey verifies a configured model has stored provider credentials. The CLI
// remains responsible for validating/refreshing those credentials with its server.
func (a Kimi) CheckKey(p agent.Profile) error {
	if p.Dir == "" {
		p.Dir = kimiHome()
	}
	if cred := kimiCredential(p.Dir); cred.token != "" {
		if cred.expired && !cred.refreshable {
			return errors.New("Kimi access token expired and no refresh credential was found")
		}
		return nil
	}
	return errors.New("Rush could not verify local Kimi credentials; check the account in Kimi")
}

func (a Kimi) SignInCommand(p agent.Profile) (*exec.Cmd, error) {
	bin := agent.Path(a.ID)
	if bin == "" {
		return nil, errors.New("Kimi CLI is not installed")
	}
	cmd := exec.Command(bin, "login")
	if p.Dir != "" {
		cmd.Env = kimiEnv(os.Environ(), p.Dir)
	}
	return cmd, nil
}

var _ agent.Authenticator = Kimi{}

func (Kimi) HistoryTail(s agent.Session, most int64) ([]event.Event, bool, error) {
	if filepath.Base(s.Transcript) == "wire.jsonl" {
		return readKimiWireFile(s.Transcript, most, time.Time{})
	}
	return ReadMessagesTail(s.Transcript, most)
}

var _ agent.TailReader = Kimi{}
