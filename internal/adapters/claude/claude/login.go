package claude

import (
	"bytes"
	"context"
	"encoding/json/jsontext"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/photon/keychain"
	"github.com/0xdeafcafe/rush/internal/state"
)

// Login is one Claude account rush can sign ~/.claude in as.
type Login = state.Login

// Vault is rush's vault, with what it does with Claude Code's sign-ins.
type Vault struct{ state.Keys }

// TheVault is rush's vault, where the logins not in use keep their
// sign-ins.
func TheVault() Vault { return Vault{state.Vault()} }

// Signed reports who a config folder is signed in as, from its state file,
// with the sign-in itself; ok is false when it isn't signed in.
func Signed(a Account) (l Login, cred []byte, ok bool) {
	prof := readProfile(a.StatePath())
	cred, err := readCreds(a)
	if prof == nil || err != nil || !usable(cred) {
		return Login{}, nil, false
	}
	var p struct {
		UUID  string `json:"accountUuid"`
		Email string `json:"emailAddress"`
		Org   string `json:"organizationName"`
	}
	if jsonx.Unmarshal(prof, &p) != nil || p.UUID == "" {
		return Login{}, nil, false
	}
	return Login{ID: p.UUID, Email: p.Email, Org: p.Org, Profile: prof}, cred, true
}

// SignedInAs is the uuid of the account a config folder is signed in as,
// from its state file alone.
func SignedInAs(a Account) string {
	var p struct {
		UUID string `json:"accountUuid"`
	}
	if prof := readProfile(a.StatePath()); prof != nil {
		_ = jsonx.Unmarshal(prof, &p)
	}
	return p.UUID
}

// Keep saves the sign-in a folder holds now into the vault, when it has
// changed: Claude Code replaces its tokens as it refreshes them, and only
// the newest works. It's kept as whoever it belongs to, which owner
// reports when that isn't the account the folder names (a Claude Code
// started before a switch put its own back); when Anthropic can't be
// asked, the folder is taken at its word.
func (v Vault) Keep(a Account) (l Login, owner string, ok bool, err error) {
	l, cred, ok := Signed(a)
	if !ok {
		return Login{}, "", false, nil
	}
	if old, err := v.Get(l.ID); err == nil && bytes.Equal(old, cred) {
		return l, "", true, nil
	}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	if id, err := Owner(ctx, cred); err == nil && id != l.ID {
		if old, err := v.Get(id); err == nil && bytes.Equal(old, cred) {
			return l, id, true, nil
		}
		return l, id, true, v.Put(id, cred)
	}
	return l, "", true, v.Put(l.ID, cred)
}

// Use signs root in as to: whatever root holds now is kept first, so
// switching back finds it as it was. A Claude Code already running keeps
// the account it started with until it restarts; when it next refreshes
// its sign-in it sees the stored one changed and takes that instead of
// writing its own back.
func (v Vault) Use(root Account, to Login) error {
	unlock, err := v.Lock()
	if err != nil {
		return err
	}
	defer unlock()
	cred, err := v.Get(to.ID)
	if err != nil || !usable(cred) {
		return fmt.Errorf("rush has no sign-in for %s; sign in to it again", to.Name)
	}
	if len(to.Profile) == 0 {
		return fmt.Errorf("rush doesn't know who %s is; sign in to it again", to.Name)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	if id, err := Owner(ctx, cred); err == nil && id != to.ID {
		// Kept under the wrong name by a rush from before it checked.
		_ = v.Forget(to.ID)
		return fmt.Errorf("rush's sign-in for %s was another account's; sign in to it again", to.Name)
	}
	if _, _, _, err := v.Keep(root); err != nil {
		return fmt.Errorf("couldn't keep the account in use: %w", err)
	}
	if now, err := readCreds(root); err == nil {
		cred = withMCPLogins(cred, now)
	}
	if err := writeCreds(root, cred); err != nil {
		return err
	}
	return writeProfile(root.StatePath(), to.Profile)
}

// Name says in a folder's state file that it's signed in as l, for a
// sign-in that is l's already.
func Name(a Account, l Login) error {
	return writeProfile(a.StatePath(), l.Profile)
}

// Adopt takes the sign-in a fresh folder was just signed in with (the one
// Login ran `claude auth login` in) into the vault, then removes the
// folder and its sign-in: the vault's copy is the only one.
func (v Vault) Adopt(scratch Account) (Login, error) {
	l, cred, ok := Signed(scratch)
	if !ok {
		return Login{}, errors.New("not signed in")
	}
	if err := v.Put(l.ID, cred); err != nil {
		return Login{}, err
	}
	_ = deleteCreds(scratch)
	_ = os.RemoveAll(scratch.ConfigDir)
	return l, nil
}

// withMCPLogins is cred carrying the MCP servers' logins now holds: Claude
// Code keeps them in the same item as the account's sign-in, but they're
// yours, not the account's, so a switch leaves them as they are rather than
// dropping them or bringing back the other account's older copies.
func withMCPLogins(cred, now []byte) []byte {
	var from map[string]jsontext.Value
	if !usable(now) || jsonx.Unmarshal(now, &from) != nil {
		return cred // nothing sound to go by: leave cred as it was
	}
	var to map[string]jsontext.Value
	if jsonx.Unmarshal(cred, &to) != nil {
		return cred
	}
	if len(from["mcpOAuth"]) == 0 {
		delete(to, "mcpOAuth") // signed out of them all
	} else {
		to["mcpOAuth"] = from["mcpOAuth"]
	}
	out, err := jsonx.Marshal(to)
	if err != nil {
		return cred
	}
	return out
}

// usable is whether a stored sign-in has what Claude Code needs to carry on
// with it: a refresh token.
func usable(cred []byte) bool {
	var c struct {
		OAuth struct {
			Refresh string `json:"refreshToken"`
		} `json:"claudeAiOauth"`
	}
	return jsonx.Unmarshal(cred, &c) == nil && c.OAuth.Refresh != ""
}

func readProfile(statePath string) jsontext.Value {
	b, err := os.ReadFile(statePath)
	if err != nil {
		return nil
	}
	var f struct {
		OAuth jsontext.Value `json:"oauthAccount"`
	}
	if jsonx.Unmarshal(b, &f) != nil || len(f.OAuth) == 0 || string(f.OAuth) == "null" {
		return nil
	}
	return f.OAuth
}

// writeProfile puts prof in the state file as who it's signed in as,
// leaving everything else. The usage Claude Code cached was the other
// account's, so it goes.
func writeProfile(statePath string, prof jsontext.Value) error {
	all := map[string]jsontext.Value{}
	b, err := os.ReadFile(statePath)
	if err != nil && !os.IsNotExist(err) {
		return err
	}
	if len(b) > 0 {
		if err := jsonx.Unmarshal(b, &all); err != nil {
			return fmt.Errorf("%s isn't readable: %w", statePath, err)
		}
	}
	all["oauthAccount"] = prof
	delete(all, "cachedUsageUtilization")
	out, err := jsonx.MarshalIndent(all)
	if err != nil {
		return err
	}
	return writeFileAtomic(statePath, out, 0o600)
}

// readCreds is the sign-in Claude Code keeps for a config folder: in the
// login keychain on macOS, in .credentials.json elsewhere.
func readCreds(a Account) ([]byte, error) {
	if runtime.GOOS == "darwin" {
		if cred, err := keychain.Read(a.keychainService(), keychain.User()); err == nil {
			return cred, nil
		}
		if cred, err := keychain.Read(a.keychainService(), ""); err == nil {
			return cred, nil
		}
		return nil, ErrNotSignedIn
	}
	return os.ReadFile(filepath.Join(a.ConfigDir, ".credentials.json"))
}

func writeCreds(a Account, cred []byte) error {
	if runtime.GOOS == "darwin" {
		return keychain.Write(a.keychainService(), keychain.User(), cred)
	}
	return writeFileAtomic(filepath.Join(a.ConfigDir, ".credentials.json"), cred, 0o600)
}

func deleteCreds(a Account) error {
	if runtime.GOOS == "darwin" {
		return keychain.Delete(a.keychainService(), keychain.User())
	}
	return os.Remove(filepath.Join(a.ConfigDir, ".credentials.json"))
}

func writeFileAtomic(path string, b []byte, mode os.FileMode) error {
	if st, err := os.Stat(path); err == nil {
		mode = st.Mode().Perm()
	}
	tmp, err := os.CreateTemp(filepath.Dir(path), "."+filepath.Base(path)+"-*")
	if err != nil {
		return err
	}
	if _, err := tmp.Write(b); err != nil {
		tmp.Close()
		os.Remove(tmp.Name())
		return err
	}
	tmp.Close()
	if err := os.Chmod(tmp.Name(), mode); err != nil {
		os.Remove(tmp.Name())
		return err
	}
	return os.Rename(tmp.Name(), path)
}
