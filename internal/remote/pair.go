package remote

import (
	"context"
	"crypto/subtle"
	"encoding/json"
	"net"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"sync"
	"time"
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
	// SSH is the command an SSH client connects with, while remote-client-ssh
	// is on; the code pairs its key. HostKey is serve's, to check it by.
	SSH     string `json:"ssh,omitempty"`
	HostKey string `json:"hostKey,omitempty"`
}

type pairCodes struct {
	mu     sync.Mutex
	codes  map[string]time.Time // code: when it stops working
	missed int
}

// pairAlphabet is a code's letters: none that read as another.
const pairAlphabet = "23456789ABCDEFGHJKLMNPQRSTUVWXYZ"

// pairMisses is how many wrong codes void every live one: a code is 40
// bits, short enough to type on a phone, so guessing is cut off instead.
const pairMisses = 10

func (p *pairCodes) issue() (string, time.Time) {
	b := randBytes(8)
	for i := range b {
		b[i] = pairAlphabet[b[i]%32]
	}
	code := string(b)
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
// It's read as typed: any case, with or without its dash or spaces.
func (p *pairCodes) take(code string) bool {
	code = strings.ToUpper(strings.NewReplacer("-", "", " ", "").Replace(code))
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
	if p.missed++; p.missed >= pairMisses {
		p.codes, p.missed = nil, 0
	}
	return false
}

// PairCode is a code as it's shown, in two halves.
func PairCode(c string) string {
	if len(c) != 8 {
		return c
	}
	return c[:4] + "-" + c[4:]
}

func (s *Server) pair(w http.ResponseWriter, r *http.Request) {
	code, until := s.pairs.issue()
	base := s.reachedAt(r.Context())
	out := Pairing{Code: code, Base: base, Expires: until}
	if base != "" {
		out.URL = base + "/#pair=" + code
	}
	if current().SSH {
		host, port, _ := net.SplitHostPort(s.Config.SSHListenOr())
		if u, err := url.Parse(base); err == nil && u.Hostname() != "" {
			host, port = u.Hostname(), strconv.Itoa(s.Config.SSHPortOr())
		}
		out.SSH = "ssh -p " + port + " " + host
		out.HostKey = SSHFingerprint()
	}
	writeJSON(w, out)
}

// reachedAt is the address phones reach this serve at: remote.json's origin,
// else the tailnet's name for it while it's reached over Tailscale.
func (s *Server) reachedAt(ctx context.Context) string {
	if s.Config.Origin != "" {
		return strings.TrimRight(s.Config.Origin, "/")
	}
	if current().Reach != ReachTailscale {
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
