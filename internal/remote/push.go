package remote

import (
	"bytes"
	"context"
	"crypto/aes"
	"crypto/cipher"
	"crypto/ecdh"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/hkdf"
	"crypto/rand"
	"crypto/sha256"
	"crypto/x509"
	"encoding/base64"
	"encoding/binary"
	"encoding/pem"
	"errors"
	"fmt"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/state"
)

// Push sends Web Push notifications (RFC 8030, encrypted as RFC 8291,
// signed as RFC 8292) to the browsers that asked for them. Its key and
// their subscriptions live in remote-push.json, readable only by you.
type Push struct {
	mu   sync.Mutex
	path string
	key  *ecdsa.PrivateKey
	// Subject is who the push services can write to about this sender.
	Subject string
	subs    []Subscription
	client  *http.Client
}

// Subscription is a browser's, as PushSubscription.toJSON gives it.
type Subscription struct {
	Endpoint string `json:"endpoint"`
	Keys     struct {
		P256dh string `json:"p256dh"`
		Auth   string `json:"auth"`
	} `json:"keys"`
}

// Notice is what one notification says, and where tapping it goes.
type Notice struct {
	Title string `json:"title"`
	Body  string `json:"body"`
	URL   string `json:"url"`
	Tag   string `json:"tag"`
}

type pushFile struct {
	Key  string         `json:"key"` // PEM, PKCS #8
	Subs []Subscription `json:"subscriptions"`
}

// LoadPush reads the sender's key and subscriptions, making a key on first use.
func LoadPush(subject string) (*Push, error) {
	p := &Push{path: filepath.Join(state.Dir(), "remote-push.json"), Subject: subject, client: &http.Client{Timeout: 20 * time.Second}}
	var f pushFile
	if b, err := os.ReadFile(p.path); err == nil {
		if err := jsonx.Unmarshal(b, &f); err != nil {
			return nil, err
		}
	}
	if blk, _ := pem.Decode([]byte(f.Key)); blk != nil {
		k, err := x509.ParsePKCS8PrivateKey(blk.Bytes)
		if err != nil {
			return nil, err
		}
		ek, ok := k.(*ecdsa.PrivateKey)
		if !ok {
			return nil, errors.New("remote-push.json: not an ECDSA key")
		}
		p.key = ek
	} else {
		k, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
		if err != nil {
			return nil, err
		}
		p.key = k
	}
	p.subs = f.Subs
	return p, p.save()
}

func (p *Push) save() error {
	der, err := x509.MarshalPKCS8PrivateKey(p.key)
	if err != nil {
		return err
	}
	b, err := jsonx.MarshalIndent(pushFile{Key: string(pem.EncodeToMemory(&pem.Block{Type: "PRIVATE KEY", Bytes: der})), Subs: p.subs})
	if err != nil {
		return err
	}
	tmp := p.path + ".tmp"
	if err := os.WriteFile(tmp, b, 0o600); err != nil {
		return err
	}
	return os.Rename(tmp, p.path)
}

var b64 = base64.RawURLEncoding

// PublicKey is the sender's key as a browser's subscribe takes it.
func (p *Push) PublicKey() string {
	pub, _ := p.key.PublicKey.ECDH()
	return b64.EncodeToString(pub.Bytes())
}

// pushHosts are the push services a subscription may name. Sending only
// to them keeps a subscription from aiming serve at anything else.
var pushHosts = []string{"fcm.googleapis.com", ".push.apple.com", ".notify.windows.com", ".push.services.mozilla.com"}

func pushHostOK(endpoint string) bool {
	u, err := url.Parse(endpoint)
	if err != nil || u.Scheme != "https" || u.User != nil || u.Port() != "" {
		return false
	}
	h := u.Hostname()
	for _, s := range pushHosts {
		if h == strings.TrimPrefix(s, ".") || (s[0] == '.' && strings.HasSuffix(h, s)) {
			return true
		}
	}
	return false
}

// Add keeps a browser's subscription, replacing one to the same endpoint.
func (p *Push) Add(s Subscription) error {
	if !pushHostOK(s.Endpoint) {
		return errors.New("not a push service rush sends to")
	}
	if k, err := b64.DecodeString(s.Keys.P256dh); err != nil || len(k) != 65 {
		return errors.New("bad p256dh key")
	}
	if a, err := b64.DecodeString(s.Keys.Auth); err != nil || len(a) != 16 {
		return errors.New("bad auth secret")
	}
	p.mu.Lock()
	defer p.mu.Unlock()
	for i := range p.subs {
		if p.subs[i].Endpoint == s.Endpoint {
			p.subs[i] = s
			return p.save()
		}
	}
	p.subs = append(p.subs, s)
	return p.save()
}

// Send sends n to every subscription, dropping those the push service
// says are gone. Delivery is the push service's best effort.
func (p *Push) Send(ctx context.Context, n Notice) {
	body, _ := jsonx.Marshal(n)
	p.mu.Lock()
	subs := append([]Subscription(nil), p.subs...)
	p.mu.Unlock()
	var gone []string
	for _, s := range subs {
		code, err := p.send(ctx, s, body)
		if err == nil && (code == http.StatusNotFound || code == http.StatusGone) {
			gone = append(gone, s.Endpoint)
		}
	}
	if len(gone) == 0 {
		return
	}
	p.mu.Lock()
	defer p.mu.Unlock()
	kept := p.subs[:0]
	for _, s := range p.subs {
		if !slices.Contains(gone, s.Endpoint) {
			kept = append(kept, s)
		}
	}
	p.subs = kept
	_ = p.save()
}

func (p *Push) send(ctx context.Context, s Subscription, payload []byte) (int, error) {
	if !pushHostOK(s.Endpoint) {
		return 0, errors.New("bad endpoint")
	}
	body, err := encrypt(s, payload)
	if err != nil {
		return 0, err
	}
	jwt, err := p.vapid(s.Endpoint)
	if err != nil {
		return 0, err
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, s.Endpoint, bytes.NewReader(body))
	if err != nil {
		return 0, err
	}
	req.Header.Set("Content-Encoding", "aes128gcm")
	req.Header.Set("Content-Type", "application/octet-stream")
	req.Header.Set("TTL", "86400")
	req.Header.Set("Urgency", "high")
	req.Header.Set("Authorization", "vapid t="+jwt+", k="+p.PublicKey())
	resp, err := p.client.Do(req)
	if err != nil {
		return 0, err
	}
	resp.Body.Close()
	return resp.StatusCode, nil
}

// vapid is the JWT (ES256) that says this sender is the one the
// subscription was made for.
func (p *Push) vapid(endpoint string) (string, error) {
	u, _ := url.Parse(endpoint)
	sub := p.Subject
	if sub == "" || !(strings.HasPrefix(sub, "https://") || strings.HasPrefix(sub, "mailto:")) {
		sub = "mailto:rush@localhost"
	}
	head := b64.EncodeToString([]byte(`{"typ":"JWT","alg":"ES256"}`))
	claims, _ := jsonx.Marshal(map[string]any{"aud": u.Scheme + "://" + u.Host, "exp": time.Now().Add(12 * time.Hour).Unix(), "sub": sub})
	unsigned := head + "." + b64.EncodeToString(claims)
	h := sha256.Sum256([]byte(unsigned))
	r, s, err := ecdsa.Sign(rand.Reader, p.key, h[:])
	if err != nil {
		return "", err
	}
	sig := make([]byte, 64)
	r.FillBytes(sig[:32])
	s.FillBytes(sig[32:])
	return unsigned + "." + b64.EncodeToString(sig), nil
}

// encrypt is payload as RFC 8291's aes128gcm for subscription s, in one record.
func encrypt(s Subscription, payload []byte) ([]byte, error) {
	uaPub, err := b64.DecodeString(s.Keys.P256dh)
	if err != nil {
		return nil, err
	}
	auth, err := b64.DecodeString(s.Keys.Auth)
	if err != nil {
		return nil, err
	}
	ua, err := ecdh.P256().NewPublicKey(uaPub)
	if err != nil {
		return nil, err
	}
	as, err := ecdh.P256().GenerateKey(rand.Reader)
	if err != nil {
		return nil, err
	}
	salt := make([]byte, 16)
	_, _ = rand.Read(salt)
	return seal(as, ua, auth, salt, payload)
}

func seal(as *ecdh.PrivateKey, ua *ecdh.PublicKey, auth, salt, payload []byte) ([]byte, error) {
	secret, err := as.ECDH(ua)
	if err != nil {
		return nil, err
	}
	asPub := as.PublicKey().Bytes()
	info := append(append([]byte("WebPush: info\x00"), ua.Bytes()...), asPub...)
	ikm, err := hkdf.Key(sha256.New, secret, auth, string(info), 32)
	if err != nil {
		return nil, err
	}
	cek, err := hkdf.Key(sha256.New, ikm, salt, "Content-Encoding: aes128gcm\x00", 16)
	if err != nil {
		return nil, err
	}
	nonce, err := hkdf.Key(sha256.New, ikm, salt, "Content-Encoding: nonce\x00", 12)
	if err != nil {
		return nil, err
	}
	blk, err := aes.NewCipher(cek)
	if err != nil {
		return nil, err
	}
	gcm, err := cipher.NewGCM(blk)
	if err != nil {
		return nil, err
	}
	if len(payload) > 3000 {
		return nil, fmt.Errorf("push payload of %d bytes is too big", len(payload))
	}
	out := make([]byte, 0, 16+4+1+65+len(payload)+17)
	out = append(out, salt...)
	out = binary.BigEndian.AppendUint32(out, 4096)
	out = append(out, byte(len(asPub)))
	out = append(out, asPub...)
	return gcm.Seal(out, nonce, append(append([]byte(nil), payload...), 2), nil), nil
}
