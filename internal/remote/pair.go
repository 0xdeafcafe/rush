package remote

import (
	"context"
	"crypto/subtle"
	"encoding/base64"
	"encoding/json"
	"net/http"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/0xdeafcafe/rush/internal/plugin"
)

// Pairing a phone: rush (with the token) asks serve for a one-time code
// and shows it as a QR code of the app's address with the code after #pair=.
// The phone opens it, and the app trades the code for the sign-in cookie at
// /api/login. The code works once, for pairTTL; the token itself is never
// shown. A fragment isn't sent in requests, so the code isn't in any log.

const pairTTL = 10 * time.Minute

// Pairing is a code to pair a phone with, and where the phone opens it.
type Pairing struct {
	Code    string    `json:"code"`
	URL     string    `json:"url"` // the app with the code, or "" when serve can't say where it's reached
	Base    string    `json:"base,omitempty"`
	Expires time.Time `json:"expires"`
}

type pairCodes struct {
	mu    sync.Mutex
	codes map[string]time.Time // code: when it stops working
}

func (p *pairCodes) issue() (string, time.Time) {
	code := base64.RawURLEncoding.EncodeToString(randBytes(16))
	until := time.Now().Add(pairTTL)
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.codes == nil {
		p.codes = map[string]time.Time{}
	}
	for c, t := range p.codes {
		if time.Now().After(t) {
			delete(p.codes, c)
		}
	}
	p.codes[code] = until
	return code, until
}

// take is whether code is a live one, using it up if so.
func (p *pairCodes) take(code string) bool {
	if code == "" {
		return false
	}
	p.mu.Lock()
	defer p.mu.Unlock()
	for c, t := range p.codes {
		if subtle.ConstantTimeCompare([]byte(c), []byte(code)) == 1 {
			delete(p.codes, c)
			return time.Now().Before(t)
		}
	}
	return false
}

func (s *Server) pair(w http.ResponseWriter, r *http.Request) {
	code, until := s.pairs.issue()
	base := s.reachedAt(r.Context())
	out := Pairing{Code: code, Base: base, Expires: until}
	if base != "" {
		out.URL = base + "/#pair=" + code
	}
	writeJSON(w, out)
}

// reachedAt is the address phones reach this serve at: remote.json's origin,
// else the tailnet's name for it while the tailscale plugin is on.
func (s *Server) reachedAt(ctx context.Context) string {
	if s.Config.Origin != "" {
		return strings.TrimRight(s.Config.Origin, "/")
	}
	if !plugin.BundledOn(PluginTailscale) {
		return ""
	}
	bin, err := tailscaleBin()
	if err != nil {
		return ""
	}
	ctx, cancel := context.WithTimeout(ctx, 5*time.Second)
	defer cancel()
	out, err := tailscale(ctx, bin, "status", "--json").Output()
	if err != nil {
		return ""
	}
	var st struct{ Self struct{ DNSName string } }
	if json.Unmarshal(out, &st) != nil || st.Self.DNSName == "" {
		return ""
	}
	return "https://" + strings.TrimSuffix(st.Self.DNSName, ".") + ":" + strconv.Itoa(s.Config.TailscalePortOr())
}
