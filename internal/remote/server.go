package remote

import (
	"context"
	"crypto/subtle"
	"embed"
	"encoding/hex"
	"encoding/json/jsontext"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"net"
	"net/http"
	"net/http/httputil"
	"net/url"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/state"
)

//go:embed web
var webFiles embed.FS

// Server is one machine's rush serve.
type Server struct {
	Config Config
	Token  string
	// Start starts a session as rush session start does.
	Start func(StartRequest) (host.Info, error)
	// Push sends notifications, when set up.
	Push *Push
}

// StartRequest is a new session as a remote client asks for it: what the
// web app's form offers, and nothing that reaches past the agent (its
// binary, environment, permission mode).
type StartRequest struct {
	Cwd     string `json:"cwd"`
	Agent   string `json:"agent,omitempty"`
	Profile string `json:"profile,omitempty"`
	Prompt  string `json:"prompt,omitempty"`
	Model   string `json:"model,omitempty"`
	Effort  string `json:"effort,omitempty"`
	Name    string `json:"name,omitempty"`
	// Resume is the session id of a past session rush didn't start (see
	// Other): it carries on in rush, with its own agent and folder.
	Resume string `json:"resume,omitempty"`
}

// Session is a session as serve lists it.
type Session struct {
	host.Info
	Alive bool `json:"alive"`
}

// Machine is one machine the web app shows: this one, or a peer.
type Machine struct {
	Name string `json:"name"`
	Self bool   `json:"self,omitzero"`
}

const cookieName = "rush_token"

// Handler is serve's whole HTTP surface.
func (s *Server) Handler() http.Handler {
	api := http.NewServeMux()
	api.HandleFunc("GET /api/machines", s.machines)
	api.HandleFunc("GET /api/sessions", s.list)
	api.HandleFunc("POST /api/sessions", s.start)
	api.HandleFunc("GET /api/sessions/{id}/events", s.events)
	api.HandleFunc("POST /api/sessions/{id}/op", s.op)
	api.HandleFunc("POST /api/git", s.git)
	api.HandleFunc("GET /api/others", s.others)
	api.HandleFunc("GET /api/push", s.pushKey)
	api.HandleFunc("POST /api/push", s.pushSubscribe)
	api.HandleFunc("/m/{name}/", s.proxy)

	web, _ := fs.Sub(webFiles, "web")
	mux := http.NewServeMux()
	mux.Handle("/", noCache(http.FileServerFS(web)))
	mux.HandleFunc("POST /api/login", s.login)
	mux.HandleFunc("POST /api/logout", logout)
	mux.Handle("/api/", s.auth(api))
	mux.Handle("/m/", s.auth(api))
	return secure(mux)
}

// secure sets headers every response carries: no framing, no sniffing,
// and scripts only from serve itself.
func secure(h http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		hd := w.Header()
		hd.Set("Content-Security-Policy", "default-src 'self'; img-src 'self' data: blob:; style-src 'self'; script-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'")
		hd.Set("X-Content-Type-Options", "nosniff")
		hd.Set("Referrer-Policy", "no-referrer")
		h.ServeHTTP(w, r)
	})
}

// noCache has the app's files checked each load, so a new rush's app
// isn't stuck behind an old one's.
func noCache(h http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Cache-Control", "no-cache")
		h.ServeHTTP(w, r)
	})
}

func (s *Server) tokenOK(t string) bool {
	return t != "" && subtle.ConstantTimeCompare([]byte(t), []byte(s.Token)) == 1
}

// auth lets through a request with the token, as a bearer (rush on
// another machine, a hub) or the browser's cookie. A browser's request
// that changes anything must be JSON from this origin: a page elsewhere
// can't send that without the browser asking first, which serve refuses.
func (s *Server) auth(h http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if b, ok := strings.CutPrefix(r.Header.Get("Authorization"), "Bearer "); ok {
			if !s.tokenOK(b) {
				fail(w, http.StatusUnauthorized, "bad token")
				return
			}
			h.ServeHTTP(w, r)
			return
		}
		c, err := r.Cookie(cookieName)
		if err != nil || !s.tokenOK(c.Value) {
			fail(w, http.StatusUnauthorized, "sign in")
			return
		}
		if r.Method != http.MethodGet && r.Method != http.MethodHead {
			if !s.sameOrigin(r) || !strings.HasPrefix(r.Header.Get("Content-Type"), "application/json") {
				fail(w, http.StatusForbidden, "cross-origin request refused")
				return
			}
		}
		h.ServeHTTP(w, r)
	})
}

// sameOrigin is whether a browser's request came from this app: its
// Origin is the configured one, or the host it asked for.
func (s *Server) sameOrigin(r *http.Request) bool {
	o := r.Header.Get("Origin")
	if o == "" {
		return r.Header.Get("Sec-Fetch-Site") == "" || r.Header.Get("Sec-Fetch-Site") == "same-origin"
	}
	if s.Config.Origin != "" && o == strings.TrimRight(s.Config.Origin, "/") {
		return true
	}
	u, err := url.Parse(o)
	return err == nil && u.Host == r.Host
}

func (s *Server) login(w http.ResponseWriter, r *http.Request) {
	var in struct {
		Token string `json:"token"`
	}
	if !s.sameOrigin(r) || readJSON(r, &in) != nil || !s.tokenOK(strings.TrimSpace(in.Token)) {
		time.Sleep(time.Second) // ponytail: a pause per bad guess, not a rate limiter; the token is 256 bits
		fail(w, http.StatusUnauthorized, "that isn't this machine's token")
		return
	}
	http.SetCookie(w, &http.Cookie{Name: cookieName, Value: s.Token, Path: "/", HttpOnly: true,
		Secure: isHTTPS(r), SameSite: http.SameSiteStrictMode, MaxAge: 400 * 24 * 3600})
	writeJSON(w, map[string]string{"name": s.Config.Name})
}

func logout(w http.ResponseWriter, r *http.Request) {
	http.SetCookie(w, &http.Cookie{Name: cookieName, Path: "/", HttpOnly: true, Secure: isHTTPS(r), MaxAge: -1})
	w.WriteHeader(http.StatusNoContent)
}

// isHTTPS is whether the browser reached serve over HTTPS, itself or
// through what's in front of it.
func isHTTPS(r *http.Request) bool {
	return r.TLS != nil || r.Header.Get("X-Forwarded-Proto") == "https"
}

func (s *Server) machines(w http.ResponseWriter, _ *http.Request) {
	out := []Machine{{Name: s.Config.Name, Self: true}}
	for _, p := range s.Config.Peers {
		out = append(out, Machine{Name: p.Name})
	}
	writeJSON(w, out)
}

func (s *Server) list(w http.ResponseWriter, _ *http.Request) {
	infos := host.List()
	out := make([]Session, 0, len(infos))
	for _, i := range infos {
		out = append(out, Session{Info: i, Alive: host.Alive(i.HostPID)})
	}
	writeJSON(w, out)
}

func (s *Server) start(w http.ResponseWriter, r *http.Request) {
	var req StartRequest
	if err := readJSON(r, &req); err != nil {
		fail(w, http.StatusBadRequest, err.Error())
		return
	}
	if req.Resume != "" {
		o, ok := pastOther(req.Resume)
		if !ok {
			fail(w, http.StatusNotFound, "no past session "+req.Resume+" on this machine")
			return
		}
		req.Agent, req.Cwd, req.Profile = o.Agent, o.Cwd, ""
	}
	if rest, ok := strings.CutPrefix(req.Cwd, "~"); ok && (rest == "" || rest[0] == '/') {
		home, _ := os.UserHomeDir()
		req.Cwd = home + rest
	}
	if !filepath.IsAbs(req.Cwd) {
		fail(w, http.StatusBadRequest, "the folder must be a full path")
		return
	}
	if err := s.allowedCwd(req.Cwd); err != nil {
		fail(w, http.StatusForbidden, err.Error())
		return
	}
	if s.Start == nil {
		fail(w, http.StatusNotImplemented, "this serve can't start sessions")
		return
	}
	info, err := s.Start(req)
	if err != nil {
		fail(w, http.StatusBadRequest, err.Error())
		return
	}
	if req.Resume != "" {
		others.Lock()
		others.at = time.Time{} // it's rush's now
		others.Unlock()
	}
	writeJSON(w, Session{Info: info, Alive: host.Alive(info.HostPID)})
}

// allowedCwd is whether a remote client may start a session in dir: never
// in rush's own folder (the tokens and keys serve holds), and where
// remote.json's workspaces list some, only in them.
func (s *Server) allowedCwd(dir string) error {
	real := func(p string) string {
		if r, err := filepath.EvalSymlinks(p); err == nil {
			return r
		}
		return filepath.Clean(p)
	}
	in := func(p, root string) bool {
		rel, err := filepath.Rel(real(root), real(p))
		return err == nil && rel != ".." && !strings.HasPrefix(rel, ".."+string(filepath.Separator))
	}
	if in(dir, state.Dir()) {
		return errors.New("that folder is rush's own")
	}
	if len(s.Config.Workspaces) == 0 {
		return nil
	}
	for _, w := range s.Config.Workspaces {
		if in(dir, w) {
			return nil
		}
	}
	return errors.New("that folder isn't one of this machine's workspaces")
}

// validID is a session id with a session folder behind it.
func validID(id string) bool {
	if id == "" || strings.ContainsAny(id, `/\.`) {
		return false
	}
	st, err := os.Stat(filepath.Join(host.Root(), id))
	return err == nil && st.IsDir()
}

// remoteOps are the ops a remote client may send a host: answering,
// steering and stopping its agent, its queue, going away and coming back,
// and what it waits on at a limit. Rewinds, compacting and relogins work on
// files or accounts the client has only on its own machine, so they stay
// with the terminal there.
var remoteOps = []string{"send", "allow", "deny", "interrupt", "stop", "stop_task", "background",
	"model", "effort", "mode", "without", "tell", "context", "limit", "away",
	"queue_edit", "queue_remove", "queue_move", "queue_merge", "queue_send", "queue_hold", "queue_separate"}

// remoteOp is the part of a host op a remote client may set: never a
// path on this machine (images, exchanges) the agent would read.
type remoteOp struct {
	Op        string         `json:"op"`
	Text      string         `json:"text,omitempty"`
	ID        string         `json:"id,omitempty"`
	Always    bool           `json:"always,omitzero"`
	Input     jsontext.Value `json:"input,omitzero"`
	Message   string         `json:"message,omitempty"`
	Interrupt bool           `json:"interrupt,omitzero"`
	Mode      string         `json:"mode,omitempty"`
	Model     string         `json:"model,omitempty"`
	Effort    string         `json:"effort,omitempty"`
	Now       bool           `json:"now,omitzero"`
	Guide     bool           `json:"guide,omitzero"`
	Index     int            `json:"index,omitzero"`
	Was       string         `json:"was,omitempty"`
	To        int            `json:"to,omitzero"`
	Without   []string       `json:"without,omitempty"`
	Away      *host.Away     `json:"away,omitempty"`
	// Pictures are a send's images as bytes: serve writes them to the
	// session's scratch folder here, so no path ever crosses.
	Pictures []Picture `json:"pictures,omitempty"`
}

// Picture is one image sent with a message.
type Picture struct {
	Type string `json:"type"` // image/png, image/jpeg, image/gif or image/webp
	Data []byte `json:"data"` // base64 in JSON
}

// pictureExt are the images an agent takes, by type.
var pictureExt = map[string]string{"image/png": ".png", "image/jpeg": ".jpg", "image/gif": ".gif", "image/webp": ".webp"}

const (
	picturesMost = 8
	pictureMost  = 20 << 20 // before the host's own 5 MB check, which a png may pass as webp
)

// savePictures writes a send's pictures into session id's scratch folder
// and gives their paths.
func savePictures(id string, ps []Picture) ([]string, error) {
	if len(ps) > picturesMost {
		return nil, fmt.Errorf("at most %d images at once", picturesMost)
	}
	var paths []string
	for _, p := range ps {
		ext := pictureExt[p.Type]
		if ext == "" {
			return nil, fmt.Errorf("%q isn't a png, jpeg, gif or webp image", p.Type)
		}
		if len(p.Data) == 0 || len(p.Data) > pictureMost {
			return nil, fmt.Errorf("an image must be under %d MB", pictureMost>>20)
		}
		dir := host.TempDir(id)
		if err := os.MkdirAll(dir, 0o700); err != nil {
			return nil, err
		}
		path := filepath.Join(dir, "remote-"+hex.EncodeToString(randBytes(8))+ext)
		if err := os.WriteFile(path, p.Data, 0o600); err != nil {
			return nil, err
		}
		paths = append(paths, path)
	}
	return paths, nil
}

// op sends one op to a session's host. A send wakes a sleeping one, as
// rush session send does; anything else needs it running. What the host
// makes of it comes back on the events stream.
func (s *Server) op(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	if !validID(id) {
		fail(w, http.StatusNotFound, "no such session")
		return
	}
	var o remoteOp
	if err := readJSONMost(r, &o, picturesMost*pictureMost*4/3+1<<20); err != nil {
		fail(w, http.StatusBadRequest, err.Error())
		return
	}
	if !slices.Contains(remoteOps, o.Op) {
		fail(w, http.StatusForbidden, fmt.Sprintf("%q can't be sent remotely", o.Op))
		return
	}
	if o.Op == "send" && strings.TrimSpace(o.Text) == "" && len(o.Pictures) == 0 {
		fail(w, http.StatusBadRequest, "nothing to send")
		return
	}
	if o.Away != nil && o.Away.Every != 0 && o.Away.Every < time.Minute {
		fail(w, http.StatusBadRequest, "check in at most once a minute")
		return
	}
	if o.Op != "send" && len(o.Pictures) > 0 {
		fail(w, http.StatusBadRequest, "only a message takes images")
		return
	}
	info, err := host.ReadInfo(id)
	if o.Op == "send" && (err != nil || !host.Alive(info.HostPID)) {
		err = host.Ensure(id)
	} else if err == nil && !host.Alive(info.HostPID) {
		err = errors.New("the session isn't running")
	}
	if err != nil {
		fail(w, http.StatusConflict, err.Error())
		return
	}
	c, err := host.Dial(id)
	if err != nil {
		fail(w, http.StatusConflict, err.Error())
		return
	}
	defer c.Close()
	images, err := savePictures(id, o.Pictures)
	if err != nil {
		fail(w, http.StatusBadRequest, err.Error())
		return
	}
	o.Pictures = nil
	b, _ := jsonx.Marshal(o)
	if err := c.Op(b, images...); err != nil {
		fail(w, http.StatusBadGateway, err.Error())
		return
	}
	w.WriteHeader(http.StatusAccepted)
}

// proxy passes a request for /m/<peer>/api/… on to that peer's serve,
// with the peer's token in place of the browser's cookie. Peers are
// remote.json's: a request can't name somewhere else to go.
func (s *Server) proxy(w http.ResponseWriter, r *http.Request) {
	name := r.PathValue("name")
	p, ok := s.Config.Peer(name)
	rest := strings.TrimPrefix(r.URL.Path, "/m/"+name)
	if !ok || !strings.HasPrefix(rest, "/api/") {
		fail(w, http.StatusNotFound, "no such machine")
		return
	}
	target, err := url.Parse(p.URL)
	if err != nil {
		fail(w, http.StatusBadGateway, "bad peer url")
		return
	}
	rp := &httputil.ReverseProxy{
		Rewrite: func(pr *httputil.ProxyRequest) {
			pr.SetURL(target)
			pr.Out.URL.Path = strings.TrimRight(target.Path, "/") + rest
			pr.Out.URL.RawPath = ""
			pr.Out.Header.Del("Cookie")
			pr.Out.Header.Del("Origin")
			pr.Out.Header.Set("Authorization", "Bearer "+p.Token)
		},
		ErrorHandler: func(w http.ResponseWriter, _ *http.Request, err error) {
			fail(w, http.StatusBadGateway, name+" can't be reached: "+err.Error())
		},
	}
	rp.ServeHTTP(w, r)
}

// events streams a session as Server-Sent Events, each {"t":…,"e":…}:
// rush's neutral events, and info, answered, sent, error and reset of
// its own. A running host gives its replay then what happens; one that
// isn't running gives its saved transcript, and the stream picks the
// host up when something wakes it. Reading never wakes it. History starts
// with the transcript's last historyMost events, or with ?full=1 all of it.
func (s *Server) events(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	if !validID(id) {
		fail(w, http.StatusNotFound, "no such session")
		return
	}
	fl, _ := w.(http.Flusher)
	h := w.Header()
	h.Set("Content-Type", "text/event-stream")
	h.Set("Cache-Control", "no-store")
	h.Set("X-Accel-Buffering", "no")
	emit := func(b []byte) { _ = sse(w, fl, b) }
	send := func(t string, e any) error {
		b, err := jsonx.Marshal(map[string]any{"t": t, "e": e})
		if err != nil {
			return nil
		}
		return sse(w, fl, b)
	}
	full := r.URL.Query().Get("full") == "1"
	ctx := r.Context()
	tick := time.NewTicker(time.Second)
	defer tick.Stop()
	var shown time.Time // the info the transcript was shown with
	for n := 0; ; n++ {
		info, err := host.ReadInfo(id)
		if err != nil {
			_ = send("error", map[string]string{"Error": err.Error()})
			return
		}
		if host.Alive(info.HostPID) {
			if c, err := host.Dial(id); err == nil {
				_ = send("reset", nil)
				if !info.ReplayFrom.IsZero() {
					sendHistory(info, info.ReplayFrom, full, emit)
				}
				err := pump(ctx, c, w, fl)
				c.Close()
				if err != nil || ctx.Err() != nil {
					return
				}
				shown = time.Time{}
			}
		} else if shown.IsZero() || !info.UpdatedAt.Equal(shown) {
			// ponytail: polls the info file each second while the host is down; fswait if that costs
			_ = send("reset", nil)
			_ = send("info", info)
			sendHistory(info, time.Time{}, full, emit)
			shown = info.UpdatedAt
			if shown.IsZero() {
				shown = time.Unix(1, 0)
			}
		} else if n%15 == 0 {
			if sse(w, fl, nil) != nil {
				return
			}
		}
		select {
		case <-ctx.Done():
			return
		case <-tick.C:
		}
	}
}

// sse writes one event, or with nil a keepalive comment.
func sse(w io.Writer, fl http.Flusher, data []byte) error {
	var err error
	if data == nil {
		_, err = io.WriteString(w, ": \n\n")
	} else {
		_, err = fmt.Fprintf(w, "data: %s\n\n", data)
	}
	if fl != nil {
		fl.Flush()
	}
	return err
}

// pump passes a host's lines on as events until the host or the client
// goes; an error is the client gone.
func pump(ctx interface{ Done() <-chan struct{} }, c *host.Client, w io.Writer, fl http.Flusher) error {
	keep := time.NewTicker(15 * time.Second)
	defer keep.Stop()
	for {
		select {
		case <-ctx.Done():
			return errors.New("gone")
		case <-keep.C:
			if err := sse(w, fl, nil); err != nil {
				return err
			}
		case l, ok := <-c.Lines:
			if !ok {
				return nil
			}
			b := wireLine(l)
			if b == nil {
				continue
			}
			if err := sse(w, fl, b); err != nil {
				return err
			}
		}
	}
}

// wireLine is a host's line as the web app reads it, nil for what it
// has no use for.
func wireLine(l []byte) []byte {
	v, err := host.Decode(l)
	if err != nil {
		return nil
	}
	var t string
	var e any
	switch v := v.(type) {
	case event.Event:
		b, err := event.Marshal(v)
		if err != nil {
			return nil
		}
		return b
	case host.InfoEvent:
		t, e = "info", v.Info
	case host.Answered:
		t, e = "answered", v
	case host.ErrorEvent:
		t, e = "error", v
	case host.Sent:
		t, e = "sent", v
	case host.Stamp:
		t, e = "stamp", v
	default:
		return nil
	}
	b, err := jsonx.Marshal(map[string]any{"t": t, "e": e})
	if err != nil {
		return nil
	}
	return b
}

// historyMost is how many of a transcript's last events a stream starts
// with, and fullMost how many when asked for all of it.
const (
	historyMost = 600
	fullMost    = 50000
)

// sendHistory sends what a session's transcript holds from before
// before (all of it when zero): its last historyMost events, or with full
// the whole transcript up to fullMost. A "cut" says there's more before.
func sendHistory(info host.Info, before time.Time, full bool, emit func([]byte)) {
	kind := agent.Migrated(info.Kind)
	p := agent.Profile{Kind: kind}
	if cfg, err := host.ReadConfig(info.ID); err == nil {
		p.Dir = cfg.Account.Dir
	}
	s := agent.Session{ID: info.SessionID, Name: info.Name, Cwd: info.Cwd, Profile: p}
	s.Transcript = agent.TranscriptPath(kind, p, info.Cwd, info.SessionID)
	var evs []event.Event
	var cut bool
	var err error
	const tailBytes = 4 << 20
	most := historyMost
	if full {
		most = fullMost
	}
	if tb, ok := agent.As[agent.HistoryTailBeforeReader](kind); ok && !before.IsZero() && !full {
		evs, cut, err = tb.HistoryTailBefore(s, tailBytes, before)
	} else if tr, ok := agent.As[agent.TailReader](kind); ok && before.IsZero() && !full {
		evs, cut, err = tr.HistoryTail(s, tailBytes)
	} else if hr, ok := agent.As[agent.HistoryReader](kind); ok {
		evs, err = hr.History(s, before) // ponytail: reads the whole transcript to keep its end
	}
	if err != nil || len(evs) == 0 {
		return
	}
	if len(evs) > most {
		evs, cut = evs[len(evs)-most:], true
	}
	if cut {
		emit([]byte(`{"t":"cut"}`))
	}
	for _, ev := range evs {
		if b, err := event.Marshal(ev); err == nil {
			emit(b)
		}
	}
}

func readJSON(r *http.Request, v any) error { return readJSONMost(r, v, 1<<20) }

func readJSONMost(r *http.Request, v any, most int64) error {
	b, err := io.ReadAll(io.LimitReader(r.Body, most+1))
	if err != nil {
		return err
	}
	if int64(len(b)) > most {
		return errors.New("that's too big to send")
	}
	return jsonx.Unmarshal(b, v)
}

func writeJSON(w http.ResponseWriter, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "no-store")
	_ = jsonx.Write(w, v)
}

func fail(w http.ResponseWriter, code int, msg string) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(code)
	_ = jsonx.Write(w, map[string]string{"error": msg})
}

// Run serves on ln until ctx ends or serving fails, with notifications
// and what the tailscale and cloudflared plugins expose running beside
// it. Whichever way it ends, ctx is cancelled before waiting on them, so
// a failed server can't leave them up or hang its own shutdown.
func (s *Server) Run(ctx context.Context, ln net.Listener, log io.Writer) error {
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	go s.Watch(ctx)
	exposed := Expose(ctx, s.Config, ln.Addr().(*net.TCPAddr).Port, log)
	hs := &http.Server{Handler: s.Handler(), ReadHeaderTimeout: 10 * time.Second}
	go func() {
		<-ctx.Done()
		sctx, c := context.WithTimeout(context.Background(), 3*time.Second)
		defer c()
		_ = hs.Shutdown(sctx)
	}()
	err := hs.Serve(ln)
	cancel()
	<-exposed
	if errors.Is(err, http.ErrServerClosed) {
		return nil
	}
	return err
}
