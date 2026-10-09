package remote

import (
	"context"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/hex"
	"errors"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/state"
)

// Device is one phone, browser or SSH key you've paired with this machine:
// each has its own credential, so each can be seen and revoked on its own.
type Device struct {
	ID       string    `json:"id"`
	Kind     string    `json:"kind"` // "web" or "ssh"
	Name     string    `json:"name"`
	Created  time.Time `json:"created"`
	LastSeen time.Time `json:"lastSeen,omitzero"`
	// TokenHash is a web device's cookie, hashed: the cookie is the token.
	TokenHash string `json:"tokenHash,omitempty"`
	// Key is an SSH device's public key, as authorized_keys writes it.
	Key         string `json:"key,omitempty"`
	Fingerprint string `json:"fingerprint,omitempty"`
	// Live is how many SSH sessions it has open now; serve fills it in.
	Live int `json:"live,omitzero"`
}

// seenEvery is how stale LastSeen may be on disk: a device in use isn't
// written each request.
const seenEvery = time.Minute

// devices is remote-devices.json in rush's folder, loaded on first use.
type devices struct {
	mu     sync.Mutex
	loaded bool
	list   []Device
}

func devicesPath() string { return filepath.Join(state.Dir(), "remote-devices.json") }

func hashToken(t string) string {
	h := sha256.Sum256([]byte(t))
	return hex.EncodeToString(h[:])
}

// load reads the file once; mu is held.
func (d *devices) load() {
	if d.loaded {
		return
	}
	d.loaded = true
	b, err := os.ReadFile(devicesPath())
	if err == nil {
		_ = jsonx.Unmarshal(b, &d.list)
	}
}

// save writes the list; mu is held.
func (d *devices) save() error {
	b, err := jsonx.MarshalIndent(d.list)
	if err != nil {
		return err
	}
	if err := os.MkdirAll(state.Dir(), 0o700); err != nil {
		return err
	}
	tmp := devicesPath() + ".tmp"
	if err := os.WriteFile(tmp, b, 0o600); err != nil {
		return err
	}
	return os.Rename(tmp, devicesPath())
}

func (d *devices) add(dev Device) (Device, error) {
	d.mu.Lock()
	defer d.mu.Unlock()
	d.load()
	dev.ID = hex.EncodeToString(randBytes(4))
	dev.Created, dev.LastSeen = time.Now(), time.Now()
	d.list = append(d.list, dev)
	return dev, d.save()
}

// addWeb makes a web device and gives the token its cookie holds.
func (d *devices) addWeb(name string) (string, Device, error) {
	tok := hex.EncodeToString(randBytes(32))
	dev, err := d.add(Device{Kind: "web", Name: name, TokenHash: hashToken(tok)})
	return tok, dev, err
}

// find is the device match picks, seen now; nil when none.
func (d *devices) find(match func(*Device) bool) *Device {
	d.mu.Lock()
	defer d.mu.Unlock()
	d.load()
	for i := range d.list {
		dev := &d.list[i]
		if !match(dev) {
			continue
		}
		if time.Since(dev.LastSeen) > seenEvery {
			dev.LastSeen = time.Now()
			_ = d.save()
		}
		out := *dev
		return &out
	}
	return nil
}

// web is the web device whose cookie tok is.
func (d *devices) web(tok string) *Device {
	if tok == "" {
		return nil
	}
	h := hashToken(tok)
	return d.find(func(dev *Device) bool {
		return dev.Kind == "web" && subtle.ConstantTimeCompare([]byte(dev.TokenHash), []byte(h)) == 1
	})
}

// ssh is the SSH device with this key (authorized_keys form).
func (d *devices) ssh(key string) *Device {
	return d.find(func(dev *Device) bool { return dev.Kind == "ssh" && dev.Key == key })
}

func (d *devices) all() []Device {
	d.mu.Lock()
	defer d.mu.Unlock()
	d.load()
	out := slices.Clone(d.list)
	slices.SortFunc(out, func(a, b Device) int { return b.LastSeen.Compare(a.LastSeen) })
	return out
}

func (d *devices) revoke(id string) bool {
	d.mu.Lock()
	defer d.mu.Unlock()
	d.load()
	n := len(d.list)
	d.list = slices.DeleteFunc(d.list, func(dev Device) bool { return dev.ID == id })
	if len(d.list) == n {
		return false
	}
	return d.save() == nil
}

// deviceName is a browser's device as people say it, from its User-Agent.
func deviceName(ua string) string {
	pick := func(pairs ...string) string {
		for i := 0; i < len(pairs); i += 2 {
			if strings.Contains(ua, pairs[i]) {
				return pairs[i+1]
			}
		}
		return ""
	}
	dev := pick("iPhone", "iPhone", "iPad", "iPad", "Android", "Android", "Macintosh", "Mac", "Windows", "Windows", "Linux", "Linux")
	app := pick("Edg/", "Edge", "Firefox/", "Firefox", "CriOS/", "Chrome", "Chrome/", "Chrome", "Safari/", "Safari")
	switch {
	case dev != "" && app != "":
		return dev + " · " + app
	case dev != "" || app != "":
		return dev + app
	}
	return "a browser"
}

func (s *Server) listDevices(w http.ResponseWriter, _ *http.Request) {
	list := s.devs.all()
	for i := range list {
		list[i].TokenHash = ""
		list[i].Live = s.live.count(list[i].ID)
	}
	writeJSON(w, list)
}

func (s *Server) revokeDevice(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	if !s.devs.revoke(id) {
		fail(w, http.StatusNotFound, "no such device")
		return
	}
	s.live.drop(id)
	writeJSON(w, struct{}{})
}

// liveConns is what each device has open now (event streams, SSH sessions),
// each ended by its cancel: revoking a device ends them at once.
type liveConns struct {
	mu sync.Mutex
	by map[string]map[*func()]bool
}

// add counts one open thing of device id's until the returned func.
func (l *liveConns) add(id string, cancel func()) (done func()) {
	l.mu.Lock()
	defer l.mu.Unlock()
	if l.by == nil {
		l.by = map[string]map[*func()]bool{}
	}
	if l.by[id] == nil {
		l.by[id] = map[*func()]bool{}
	}
	k := &cancel
	l.by[id][k] = true
	return func() {
		l.mu.Lock()
		defer l.mu.Unlock()
		delete(l.by[id], k)
	}
}

func (l *liveConns) count(id string) int {
	l.mu.Lock()
	defer l.mu.Unlock()
	return len(l.by[id])
}

func (l *liveConns) drop(id string) {
	l.mu.Lock()
	open := l.by[id]
	delete(l.by, id)
	l.mu.Unlock()
	for c := range open {
		(*c)()
	}
}

// LocalDevices lists the devices paired with this machine, through its serve.
func LocalDevices(ctx context.Context) ([]Device, error) {
	var list []Device
	err := localCall(ctx, http.MethodGet, "/api/devices", &list)
	return list, err
}

// LocalRevoke unpairs a device, ending what it has open.
func LocalRevoke(ctx context.Context, id string) error {
	var out struct{}
	return localCall(ctx, http.MethodPost, "/api/devices/"+id+"/revoke", &out)
}

// localCall is one request to this machine's serve, with its token.
func localCall(ctx context.Context, method, path string, out any) error {
	cfg, err := LoadConfig()
	if err != nil {
		return err
	}
	tok, err := Token()
	if err != nil {
		return err
	}
	ctx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, method, "http://"+cfg.Listen+path, strings.NewReader("{}"))
	if err != nil {
		return err
	}
	req.Header.Set("Authorization", "Bearer "+tok)
	req.Header.Set("Content-Type", "application/json")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return errors.New("rush serve isn't answering: turn Remote mode on")
	}
	defer resp.Body.Close()
	b, err := io.ReadAll(io.LimitReader(resp.Body, 4<<20))
	if err != nil {
		return err
	}
	if resp.StatusCode != http.StatusOK {
		var e struct{ Error string }
		_ = jsonx.Unmarshal(b, &e)
		return errors.New(or(e.Error, resp.Status))
	}
	return jsonx.Unmarshal(b, out)
}
