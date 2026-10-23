package ts

import (
	"context"
	"errors"
	"fmt"
	"os"
	"sync"
	"time"

	bgate "github.com/0xdeafcafe/rush/internal/bundled/gate"
	"github.com/0xdeafcafe/rush/internal/gate"
)

// Options are the pool's, from the plugin's settings.
type Options struct {
	Engine string        // "auto", "tsrs" or "tsgo"
	Cap    uint64        // bytes one server may hold; 0 for no cap
	Idle   time.Duration // a server asked nothing this long is closed; 0 never
	// Log records what the pool did to a server and why.
	Log func(kind string, fields map[string]any)
}

// A Pool is the servers this plugin runs: one per checkout, shared by
// every agent that asks about it.
type Pool struct {
	mu       sync.Mutex
	opts     Options
	servers  map[string]*Server
	starting map[string]chan struct{}
}

// NewPool is a pool that watches its servers until ctx ends.
func NewPool(ctx context.Context, o Options) *Pool {
	p := &Pool{opts: o, servers: map[string]*Server{}, starting: map[string]chan struct{}{}}
	go p.watch(ctx)
	return p
}

// SetOptions changes the options, for servers started from now on and
// the next look at those running.
func (p *Pool) SetOptions(o Options) {
	p.mu.Lock()
	p.opts = o
	p.mu.Unlock()
}

func (p *Pool) options() Options {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.opts
}

// startWait is how long a call waits for memory for a new server.
const startWait = 45 * time.Second

// For is the server for the checkout file is in, started if need be.
func (p *Pool) For(ctx context.Context, file string) (*Server, error) {
	root := WorkspaceRoot(file)
	if root == "" {
		return nil, fmt.Errorf("%s is in no checkout and under no tsconfig.json", file)
	}
	for {
		p.mu.Lock()
		if s := p.servers[root]; s != nil && s.Why() == nil {
			p.mu.Unlock()
			return s, nil
		}
		if wait, ok := p.starting[root]; ok {
			p.mu.Unlock()
			select {
			case <-wait:
				continue
			case <-ctx.Done():
				return nil, ctx.Err()
			}
		}
		wait := make(chan struct{})
		p.starting[root] = wait
		p.mu.Unlock()
		s, err := p.start(ctx, root)
		p.mu.Lock()
		delete(p.starting, root)
		if err == nil {
			p.servers[root] = s
		}
		p.mu.Unlock()
		close(wait)
		return s, err
	}
}

// start runs a server for root once the gate has memory for it.
func (p *Pool) start(ctx context.Context, root string) (*Server, error) {
	e, err := Resolve(root, p.options().Engine)
	if err != nil {
		return nil, err
	}
	var run *gate.Run
	// Inside a gated run (a test of this, say) the gate lets it through,
	// as it does everything else there.
	if rules := bgate.Rules(); rules != nil && os.Getenv(gate.HeldEnv) == "" {
		rule, ok := rules[e.Name]
		if !ok {
			rule = gate.Rule{Memory: gate.Estimates["tsrs"]}
		}
		rule.Limit = 0 // a server lives as long as it's used
		wctx, cancel := context.WithTimeout(ctx, startWait)
		run, err = gate.Acquire(wctx, gate.Ask{Name: e.Name, Rule: rule, Session: "rush ts", Reserve: bgate.Reserve()}, nil)
		cancel()
		if err != nil {
			return nil, fmt.Errorf("no memory for a %s language server for %s after %s: the machine is busy; try again "+
				"once other runs finish (rush gate status)", e.Name, root, startWait)
		}
	}
	s, err := Start(ctx, root, e, run)
	if err != nil {
		if run != nil {
			run.Release()
		}
		return nil, err
	}
	p.log("ts-started", map[string]any{"root": root, "engine": e.Name, "bin": e.Bin, "server": s.PID()})
	return s, nil
}

// Servers is the servers running, by checkout.
func (p *Pool) Servers() map[string]*Server {
	p.mu.Lock()
	defer p.mu.Unlock()
	out := make(map[string]*Server, len(p.servers))
	for root, s := range p.servers {
		out[root] = s
	}
	return out
}

// CloseAll closes every server.
func (p *Pool) CloseAll() {
	for root, s := range p.Servers() {
		s.Close()
		p.drop(root, s)
	}
}

func (p *Pool) drop(root string, s *Server) {
	p.mu.Lock()
	if p.servers[root] == s {
		delete(p.servers, root)
	}
	p.mu.Unlock()
}

// watch looks at each server every two seconds: what it holds goes on its
// gate ticket, one past its cap is stopped, one idle too long closed, and
// one that died is dropped, so the next call starts another.
func (p *Pool) watch(ctx context.Context) {
	t := time.NewTicker(2 * time.Second)
	defer t.Stop()
	for {
		select {
		case <-ctx.Done():
			p.CloseAll()
			return
		case <-t.C:
		}
		o := p.options()
		for root, s := range p.Servers() {
			if why := s.Why(); why != nil {
				p.drop(root, s)
				p.log("ts-gone", map[string]any{"root": root, "why": why.Error()})
				continue
			}
			held := s.Footprint()
			if s.run != nil {
				s.run.Note(held)
			}
			switch {
			case o.Cap > 0 && held > o.Cap:
				why := capError(s, held, o.Cap)
				s.Kill(why)
				p.drop(root, s)
				p.log("ts-capped", map[string]any{"root": root, "engine": s.Engine.Name, "footprint": held, "cap": o.Cap})
			case o.Idle > 0 && s.Idle() > o.Idle:
				s.Close()
				p.drop(root, s)
				p.log("ts-idle", map[string]any{"root": root, "idle": o.Idle.String()})
			}
		}
	}
}

// capError is what a call in flight hears when its server passed its cap.
func capError(s *Server, held, max uint64) error {
	return errors.New(fmt.Sprintf("the %s language server for %s held %s, past its %s cap, so rush stopped it. "+
		"Narrow the query: give file and line rather than a bare symbol search (that loads every project), "+
		"and work one package at a time. The next call starts a fresh server.",
		s.Engine.Name, s.Root, gate.Size(held), gate.Size(max)))
}

func (p *Pool) log(kind string, f map[string]any) {
	if log := p.options().Log; log != nil {
		log(kind, f)
	}
}
