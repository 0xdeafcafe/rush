package codex

import (
	"bytes"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/state"
)

// Codex keeps its sign-in in its home's auth.json: a ChatGPT sign-in's
// tokens, or an API key. An account rush keeps is a copy of that file in
// the vault, and switching puts one in place of the other.

// vaultKey is where the vault keeps an account's auth.json.
func vaultKey(id string) string { return "codex:" + id }

// authFile is Codex's auth.json, as much as rush reads of it.
type authFile struct {
	Mode   string  `json:"auth_mode"`
	APIKey *string `json:"OPENAI_API_KEY"`
	Tokens *struct {
		IDToken   string `json:"id_token"`
		AccountID string `json:"account_id"`
	} `json:"tokens"`
}

// whoIs is the account an auth.json signs in as. It never returns a
// secret: an API key's account is named by a hash of it.
func whoIs(b []byte) (agent.Account, error) {
	var f authFile
	if err := jsonx.Unmarshal(b, &f); err != nil {
		return agent.Account{}, errors.New("codex: its auth.json isn't readable")
	}
	a := agent.Account{Kind: Kind}
	switch {
	case f.Tokens != nil && f.Tokens.AccountID != "":
		a.ID = f.Tokens.AccountID
		claims := jwtClaims(f.Tokens.IDToken)
		a.Email = claims.Email
		a.Plan = claims.Auth.Plan
	case f.APIKey != nil && *f.APIKey != "":
		sum := sha256.Sum256([]byte(*f.APIKey))
		a.ID, a.Name, a.Plan = "apikey-"+hex.EncodeToString(sum[:4]), "API key", "API key"
	default:
		return agent.Account{}, errors.New("codex: not signed in (codex login)")
	}
	a.Key = "codex:" + a.ID
	return a, nil
}

type idClaims struct {
	Email string `json:"email"`
	Auth  struct {
		Plan string `json:"chatgpt_plan_type"`
	} `json:"https://api.openai.com/auth"`
}

// jwtClaims reads who an id token names; it isn't checked, only read.
func jwtClaims(tok string) idClaims {
	var c idClaims
	parts := strings.Split(tok, ".")
	if len(parts) < 2 {
		return c
	}
	b, err := base64.RawURLEncoding.DecodeString(parts[1])
	if err == nil {
		_ = jsonx.Unmarshal(b, &c)
	}
	return c
}

func authPath(p agent.Profile) string { return filepath.Join(p.Dir, "auth.json") }

// Current is who p's auth.json signs in as.
func (Adapter) Current(p agent.Profile) (agent.Account, error) {
	b, err := os.ReadFile(authPath(p))
	if err != nil {
		return agent.Account{}, errors.New("codex: no auth.json; not signed in, or its sign-in is in the keyring, which rush can't switch")
	}
	return whoIs(b)
}

// Switch puts a's auth.json in p, keeping p's own first: Codex refreshes
// its tokens as it goes, and only the newest work.
func (Adapter) Switch(p agent.Profile, a agent.Account) error {
	v := state.Vault()
	unlock, err := v.Lock()
	if err != nil {
		return err
	}
	defer unlock()
	next, err := v.Get(vaultKey(a.ID))
	if err != nil {
		return fmt.Errorf("rush has no sign-in for %s; sign in to it again", firstOf(a.Name, a.Email, a.ID))
	}
	if who, err := whoIs(next); err != nil || who.ID != a.ID {
		return fmt.Errorf("rush's sign-in for %s isn't that account's; sign in to it again", firstOf(a.Name, a.Email, a.ID))
	}
	if cur, err := os.ReadFile(authPath(p)); err == nil {
		if who, err := whoIs(cur); err == nil {
			if old, err := v.Get(vaultKey(who.ID)); err != nil || !bytes.Equal(old, cur) {
				if err := v.Put(vaultKey(who.ID), cur); err != nil {
					return fmt.Errorf("couldn't keep the account in use: %w", err)
				}
			}
		}
	}
	return writeAtomic(authPath(p), next)
}

// Keep saves p's auth.json in the vault as whoever it signs in as, when
// it has changed.
func Keep(p agent.Profile) (agent.Account, error) {
	b, err := os.ReadFile(authPath(p))
	if err != nil {
		return agent.Account{}, err
	}
	who, err := whoIs(b)
	if err != nil {
		return agent.Account{}, err
	}
	v := state.Vault()
	if old, err := v.Get(vaultKey(who.ID)); err == nil && bytes.Equal(old, b) {
		return who, nil
	}
	return who, v.Put(vaultKey(who.ID), b)
}

// SignIn is `codex login` in a home of its own, which keeps its sign-in
// in a file there; done takes it into the vault and removes the home.
func (Adapter) SignIn(agent.Profile) (*exec.Cmd, func() (agent.Account, error), error) {
	b := make([]byte, 4)
	_, _ = rand.Read(b)
	home := filepath.Join(state.Dir(), "signin-codex-"+hex.EncodeToString(b))
	if err := os.MkdirAll(home, 0o700); err != nil {
		return nil, nil, err
	}
	if err := os.WriteFile(filepath.Join(home, "config.toml"), []byte("cli_auth_credentials_store = \"file\"\n"), 0o600); err != nil {
		return nil, nil, err
	}
	bin := "codex"
	if _, err := exec.LookPath(bin); err != nil {
		if p := agent.Path(Kind); p != "" {
			bin = p
		}
	}
	cmd := exec.Command(bin, "login")
	cmd.Env = append(os.Environ(), "CODEX_HOME="+home)
	done := func() (agent.Account, error) {
		defer os.RemoveAll(home)
		b, err := os.ReadFile(filepath.Join(home, "auth.json"))
		if err != nil {
			return agent.Account{}, errors.New("not signed in")
		}
		who, err := whoIs(b)
		if err != nil {
			return agent.Account{}, err
		}
		return who, state.Vault().Put(vaultKey(who.ID), b)
	}
	return cmd, done, nil
}

// Forget drops rush's copy of a's auth.json.
func (Adapter) Forget(a agent.Account) error {
	return state.Vault().Forget(vaultKey(a.ID))
}

func writeAtomic(path string, b []byte) error {
	tmp, err := os.CreateTemp(filepath.Dir(path), ".auth-*.json")
	if err != nil {
		return err
	}
	_, err = tmp.Write(b)
	if cerr := tmp.Close(); err == nil {
		err = cerr
	}
	if err == nil {
		err = os.Chmod(tmp.Name(), 0o600)
	}
	if err != nil {
		os.Remove(tmp.Name())
		return err
	}
	return os.Rename(tmp.Name(), path)
}

func firstOf(vs ...string) string {
	for _, v := range vs {
		if v != "" {
			return v
		}
	}
	return ""
}

var _ agent.Accounts = Adapter{}
