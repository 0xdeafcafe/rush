package main

import (
	"bufio"
	"bytes"
	"context"
	"errors"
	"flag"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"os/signal"
	"strings"
	"syscall"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/remote"
)

const remoteUsage = `rush serve [--listen ADDR]
        serve this machine's sessions to the web app and to rush elsewhere;
        127.0.0.1:7878 unless remote.json or --listen says. See docs/remote.md.
  rush remote token                    this machine's serve token
  rush remote add NAME URL TOKEN       show another machine's serve here
  rush remote attach NAME              keep NAME's sessions in this machine's rush (list, view, send,
                                       answer, stop) until it ends; start them with rush session --on NAME start
  rush remote remove NAME
  rush remote list
  rush session --on NAME list|info|start|send|interrupt|stop|watch …
        the session commands, run on machine NAME
`

// serveCmd runs rush serve until it's interrupted.
func serveCmd(args []string) error {
	fs := flag.NewFlagSet("serve", flag.ContinueOnError)
	listen := fs.String("listen", "", "")
	if err := fs.Parse(args); err != nil {
		return err
	}
	cfg, err := remote.LoadConfig()
	if err != nil {
		return err
	}
	if *listen != "" {
		cfg.Listen = *listen
	}
	tok, err := remote.Token()
	if err != nil {
		return err
	}
	push, err := remote.LoadPush(cfg.Origin)
	if err != nil {
		return err
	}
	srv := &remote.Server{Config: cfg, Token: tok, Push: push, Start: func(r remote.StartRequest) (host.Info, error) {
		return startSession(startOpts{Cwd: r.Cwd, Kind: r.Agent, Profile: r.Profile, Prompt: r.Prompt,
			Model: r.Model, Effort: r.Effort, Name: r.Name, SessionID: r.Resume, Resume: r.Resume != ""})
	}}
	ln, err := net.Listen("tcp", cfg.Listen)
	if err != nil {
		return err
	}
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	fmt.Fprintf(os.Stderr, "rush serve: %s on http://%s (rush remote token shows the token)\n", cfg.Name, ln.Addr())
	return srv.Run(ctx, ln, os.Stderr)
}

func remoteCmd(args []string, stdout io.Writer) error {
	cfg, err := remote.LoadConfig()
	if err != nil {
		return err
	}
	sub := ""
	if len(args) > 0 {
		sub = args[0]
	}
	switch {
	case sub == "token":
		tok, err := remote.Token()
		fmt.Fprintln(stdout, tok)
		return err
	case sub == "list":
		for _, p := range cfg.Peers {
			fmt.Fprintf(stdout, "%s  %s\n", p.Name, p.URL)
		}
		return nil
	case sub == "add" && len(args) == 4:
		name, u, tok := args[1], strings.TrimRight(args[2], "/"), args[3]
		if !strings.HasPrefix(u, "https://") && !strings.HasPrefix(u, "http://") {
			return errors.New("the URL starts https:// (or http:// over a tailnet)")
		}
		if name == "" || strings.ContainsAny(name, "/ ") {
			return errors.New("a machine's name has no spaces or slashes")
		}
		kept := cfg.Peers[:0]
		for _, p := range cfg.Peers {
			if p.Name != name {
				kept = append(kept, p)
			}
		}
		cfg.Peers = append(kept, remote.Peer{Name: name, URL: u, Token: tok})
		return remote.SaveConfig(cfg)
	case sub == "attach" && len(args) == 2:
		p, ok := cfg.Peer(args[1])
		if !ok {
			return fmt.Errorf("no machine %q: rush remote add it first", args[1])
		}
		ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
		defer stop()
		fmt.Fprintf(os.Stderr, "rush remote attach: %s's sessions are in rush here until this ends\n", p.Name)
		return remote.Attach(ctx, p, os.Stderr)
	case sub == "remove" && len(args) == 2:
		kept := cfg.Peers[:0]
		for _, p := range cfg.Peers {
			if p.Name != args[1] {
				kept = append(kept, p)
			}
		}
		cfg.Peers = kept
		return remote.SaveConfig(cfg)
	}
	fmt.Fprint(stdout, remoteUsage)
	return nil
}

// remoteSessionCmd is rush session --on NAME: the session commands, run
// through that machine's serve.
func remoteSessionCmd(name string, args []string, stdin io.Reader, stdout io.Writer) (bool, error) {
	cfg, err := remote.LoadConfig()
	if err != nil {
		return false, err
	}
	p, ok := cfg.Peer(name)
	if !ok {
		return false, fmt.Errorf("no machine %q: rush remote add it first", name)
	}
	if len(args) == 0 {
		return false, errors.New(sessionUsage)
	}
	sub, rest := args[0], args[1:]
	asJSON := hasFlag(rest, "--json")
	fs := newFlags(sub)
	fs.Bool("json", false, "")
	call := func(method, path string, in, out any) error {
		var body io.Reader
		if in != nil {
			b, _ := jsonx.Marshal(in)
			body = bytes.NewReader(b)
		}
		req, err := http.NewRequest(method, p.URL+path, body)
		if err != nil {
			return err
		}
		req.Header.Set("Authorization", "Bearer "+p.Token)
		req.Header.Set("Content-Type", "application/json")
		resp, err := http.DefaultClient.Do(req)
		if err != nil {
			return err
		}
		defer resp.Body.Close()
		b, _ := io.ReadAll(resp.Body)
		if resp.StatusCode >= 300 {
			var e struct{ Error string }
			if jsonx.Unmarshal(b, &e) == nil && e.Error != "" {
				return fmt.Errorf("%s: %s", name, e.Error)
			}
			return fmt.Errorf("%s: %s", name, resp.Status)
		}
		if out != nil {
			return jsonx.Unmarshal(b, out)
		}
		return nil
	}
	show := func(s remote.Session) {
		if asJSON {
			writeJSON(stdout, s)
		} else {
			fmt.Fprintf(stdout, "%s  %-8s %s\n", s.ID, s.State, s.Name)
		}
	}
	switch sub {
	case "list":
		if hasFlag(rest, "--others") {
			var list []remote.Other
			if err := call("GET", "/api/others", nil, &list); err != nil {
				return asJSON, err
			}
			if asJSON {
				writeJSON(stdout, list)
				return true, nil
			}
			for _, o := range list {
				state := "past"
				if o.Live {
					state = "running"
				}
				fmt.Fprintf(stdout, "%s  %-8s %-7s %s  %s\n", o.SessionID, o.Agent, state, o.Name, o.Cwd)
			}
			return false, nil
		}
		var list []remote.Session
		if err := call("GET", "/api/sessions", nil, &list); err != nil {
			return asJSON, err
		}
		if asJSON {
			writeJSON(stdout, list)
			return true, nil
		}
		for _, s := range list {
			show(s)
		}
		return false, nil
	case "info":
		id, err := idAndFlags(fs, rest)
		if err != nil {
			return asJSON, err
		}
		var list []remote.Session
		if err := call("GET", "/api/sessions", nil, &list); err != nil {
			return asJSON, err
		}
		for _, s := range list {
			if s.ID == id {
				show(s)
				return asJSON, nil
			}
		}
		return asJSON, fmt.Errorf("session %s %w", id, errNotFound)
	case "start":
		var r remote.StartRequest
		var promptFile string
		fs.StringVar(&r.Cwd, "cwd", "", "")
		fs.StringVar(&r.Agent, "agent", "", "")
		fs.StringVar(&r.Profile, "profile", "", "")
		fs.StringVar(&r.Name, "name", "", "")
		fs.StringVar(&r.Model, "model", "", "")
		fs.StringVar(&r.Effort, "effort", "", "")
		fs.StringVar(&r.Resume, "resume", "", "")
		fs.StringVar(&promptFile, "prompt-file", "", "")
		if err := fs.Parse(rest); err != nil {
			return asJSON, err
		}
		if promptFile != "" {
			var b []byte
			if promptFile == "-" {
				b, err = io.ReadAll(stdin)
			} else {
				b, err = os.ReadFile(promptFile)
			}
			if err != nil {
				return asJSON, err
			}
			r.Prompt = strings.TrimRight(string(b), "\n")
		}
		var s remote.Session
		if err := call("POST", "/api/sessions", r, &s); err != nil {
			return asJSON, err
		}
		show(s)
		return asJSON, nil
	case "send", "interrupt", "stop":
		now := fs.Bool("now", false, "")
		id, err := idAndFlags(fs, rest)
		if err != nil {
			return asJSON, err
		}
		op := map[string]any{"op": sub}
		if sub == "send" {
			b, err := io.ReadAll(stdin)
			if err != nil {
				return asJSON, err
			}
			op["text"], op["now"] = strings.TrimRight(string(b), " \t\r\n"), *now
		}
		return asJSON, call("POST", "/api/sessions/"+id+"/op", op, nil)
	case "watch":
		id, err := idAndFlags(fs, rest)
		if err != nil {
			return asJSON, err
		}
		return asJSON, remoteWatch(p, id, asJSON, stdout)
	}
	return asJSON, fmt.Errorf("unknown session command %q", sub)
}

// remoteWatch prints a remote session's events as they come: each as a
// JSON line with --json, else what's said.
func remoteWatch(p remote.Peer, id string, asJSON bool, stdout io.Writer) error {
	req, err := http.NewRequest("GET", p.URL+"/api/sessions/"+id+"/events", nil)
	if err != nil {
		return err
	}
	req.Header.Set("Authorization", "Bearer "+p.Token)
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("%s: %s", p.Name, resp.Status)
	}
	sc := bufio.NewScanner(resp.Body)
	sc.Buffer(nil, 64<<20)
	for sc.Scan() {
		data, ok := strings.CutPrefix(sc.Text(), "data: ")
		if !ok {
			continue
		}
		if asJSON {
			fmt.Fprintln(stdout, data)
			continue
		}
		var ev struct {
			T string `json:"t"`
			E struct {
				Role  string
				Parts []struct {
					Kind int
					Text string
				}
				Reason, Err string
			} `json:"e"`
		}
		if jsonx.Unmarshal([]byte(data), &ev) != nil {
			continue
		}
		switch ev.T {
		case "message":
			for _, pt := range ev.E.Parts {
				if pt.Kind == 0 && pt.Text != "" {
					fmt.Fprintf(stdout, "%s: %s\n", ev.E.Role, pt.Text)
				}
			}
		case "turn_end":
			fmt.Fprintf(stdout, "— turn %s %s\n", ev.E.Reason, ev.E.Err)
		}
	}
	return sc.Err()
}
