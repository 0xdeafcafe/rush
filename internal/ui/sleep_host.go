package ui

import (
	"fmt"
	"slices"

	tea "charm.land/bubbletea/v2"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
)

type sleepDraft struct {
	input  []rune
	back   int
	imgs   imageRefs
	pastes pastes
}

// parkHost releases only the socket. The visible conversation and composer
// belong to the UI, not to the lifetime of an idle worker process.
func (m *Model) parkHost(c *hostConn) {
	if c.client != nil {
		cl := c.client
		go cl.Close()
	} //nolint:errcheck
	c.client = nil
	c.sleeping = true
	c.waking = false
	c.ready = true
}

// carryHostView keeps user-owned state when a sleeping session reconnects.
// A new connection owner prevents late reads from its old socket replacing it.
func carryHostView(old, next *hostConn) {
	next.sess, next.ready = old.sess, old.ready
	if next.pre != nil && next.pre.info != nil {
		next.sess.Info = *next.pre.info
	}
	next.input, next.back, next.anchor = old.input, old.back, old.anchor
	next.imgs, next.pastes, next.undo = old.imgs, old.pastes, old.undo
	next.view, next.sel, next.scroll = old.view, old.sel, old.scroll
	next.open, next.looks, next.verbose, next.depth, next.historyMode = old.open, old.looks, old.verbose, old.depth, old.historyMode
	next.top = old.top
	next.picked, next.sending, next.landed, next.lastSend = old.picked, old.sending, old.landed, old.lastSend
	next.coldOK = old.coldOK
	next.sleepDraft = old.sleepDraft
	next.backShown = old.backShown
	old.unwatch()
	old.closed.Store(true)
}

// wakeHost runs once for the explicitly submitted message. Ensure serializes
// process starts; the prompt is sent over its new socket exactly once, never
// also placed in a startup configuration.
func (m *Model) wakeHost(c *hostConn, a *fleet.Agent, text string, images []string, now bool) tea.Cmd {
	return m.reconnectSleeping(c, a, text, images, now, nil)
}

// wakeHostThen wakes only the host, without a message or model request.
func (m *Model) wakeHostThen(c *hostConn, after func(*Model, *hostConn) tea.Cmd) tea.Cmd {
	a := m.agentByKey(c.key)
	if a == nil || !a.Rush {
		m.flash("The saved Rush session is unavailable", true)
		return nil
	}
	if c.waking {
		m.flash("Host is waking; try the setting again once connected", false)
		return nil
	}
	return m.reconnectSleeping(c, a, "", nil, false, after)
}

func (m *Model) reconnectSleeping(c *hostConn, a *fleet.Agent, text string, images []string, now bool, after func(*Model, *hostConn) tea.Cmd) tea.Cmd {
	if c.waking {
		return nil
	}
	c.waking = true
	copyAgent := *a
	m.flash("waking "+a.DisplayName+"…", false)
	if after == nil {
		m.markSending(c, text)
	}
	return sheetDo(func() (hostOpenMsg, error) {
		if err := host.Ensure(copyAgent.ID); err != nil {
			return hostOpenMsg{}, err
		}
		msg, ok := openHost(&copyAgent)().(hostOpenMsg)
		if !ok {
			return hostOpenMsg{}, fmt.Errorf("could not reconnect to session")
		}
		if msg.err != nil {
			return msg, msg.err
		}
		if msg.c == nil || msg.c.client == nil {
			return msg, fmt.Errorf("session is still asleep; message was not sent")
		}
		cl := msg.c.client
		if after == nil {
			if err := cl.SendImages(text, images, now); err != nil {
				_ = cl.Close()
				return hostOpenMsg{}, err
			}
		}
		return msg, nil
	}, func(m *Model, msg hostOpenMsg, err error) tea.Cmd {
		c.waking = false
		if m.host != c {
			if msg.c != nil && msg.c.client != nil {
				go msg.c.client.Close()
			} //nolint:errcheck
			return nil
		}
		if err != nil {
			if after != nil {
				m.flash("Could not wake host: "+err.Error(), true)
				return nil
			}
			m.sendFailed(sendFailedMsg{key: c.key, err: err})
			if len(c.input) == 0 {
				if d := c.sleepDraft; d != nil {
					c.input, c.back, c.imgs, c.pastes = d.input, d.back, d.imgs, d.pastes
				} else {
					c.input = []rune(text)
					for _, path := range images {
						c.input = append(c.input, []rune(" "+c.imgs.add(path))...)
					}
				}
			}
			c.sleepDraft = nil
			return nil
		}
		c.sleepDraft = nil
		m.hostOpening = c.key
		replay := m.onHostOpen(msg)
		if after != nil {
			return tea.Batch(replay, after(m, m.host))
		}
		return replay
	})
}

func (c *hostConn) rememberSleepDraft() {
	if c.sleeping {
		c.sleepDraft = &sleepDraft{input: slices.Clone(c.input), back: c.back, imgs: c.imgs.clone(), pastes: c.pastes}
	}
}

// EOF may beat the final socket Info. Check its persisted marker off the UI
// thread before treating the connection as an unexpected loss.
func (m *Model) resolveHostClose(c *hostConn) tea.Cmd {
	c.waking = true // retain input until the marker check resolves
	id := c.id
	return sheetDo(func() (host.Info, error) { return host.ReadInfo(id) }, func(m *Model, info host.Info, err error) tea.Cmd {
		if m.host != c {
			return nil
		}
		c.waking = false
		if err == nil && info.Sleeping {
			c.sess.Info = info
			m.parkHost(c)
			return nil
		}
		opening := m.hostOpening
		m.dropHost()
		m.hostOpening = opening
		return nil
	})
}

// openSavedHost reads a dormant Rush session, including its effective setup,
// without a socket or any wake attempt. An empty transcript is still a session.
func openSavedHost(a *fleet.Agent) tea.Cmd {
	snapshot := *a
	return func() tea.Msg {
		info, err := host.ReadInfo(snapshot.ID)
		if err == nil {
			snapshot.SessionID = info.SessionID
			if snapshot.Kind == "" {
				snapshot.Kind = info.Kind
			}
			if snapshot.Cwd == "" {
				snapshot.Cwd = info.Cwd
			}
		}
		msg := openTail(&snapshot)().(hostOpenMsg)
		if msg.c != nil && err == nil {
			msg.c.sess.Info = info
			msg.c.sleeping = info.Sleeping
		}
		return msg
	}
}
