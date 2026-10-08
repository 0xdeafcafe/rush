package state

import (
	"encoding/json/jsontext"
	"os"
	"path/filepath"
	"runtime"
	"syscall"

	"github.com/0xdeafcafe/photon/keychain"
	"github.com/0xdeafcafe/rush/internal/agent"
)

// Keys keeps the sign-ins of accounts not in use: in the login keychain on
// macOS, in files only you can read elsewhere.
type Keys struct{ Dir string }

const vaultService = "rush-login"

// Vault is where rush keeps the sign-ins of the logins not in use.
func Vault() Keys { return Keys{Dir: filepath.Join(Dir(), "logins")} }

func (v Keys) Get(id string) ([]byte, error) {
	if runtime.GOOS == "darwin" {
		return keychain.Read(vaultService, id)
	}
	return os.ReadFile(filepath.Join(v.Dir, id+".json"))
}

func (v Keys) Put(id string, cred []byte) error {
	if runtime.GOOS == "darwin" {
		return keychain.Write(vaultService, id, cred)
	}
	if err := os.MkdirAll(v.Dir, 0o700); err != nil {
		return err
	}
	return writeBytes(filepath.Join(v.Dir, id+".json"), cred)
}

func (v Keys) Forget(id string) error {
	if runtime.GOOS == "darwin" {
		return keychain.Delete(vaultService, id)
	}
	return os.Remove(filepath.Join(v.Dir, id+".json"))
}

// Lock keeps two rushes from switching any agent's account at once; the
// func it returns lets go.
func (v Keys) Lock() (func(), error) {
	if err := os.MkdirAll(v.Dir, 0o700); err != nil {
		return nil, err
	}
	f, err := os.OpenFile(filepath.Join(v.Dir, ".lock"), os.O_CREATE|os.O_RDWR, 0o600)
	if err != nil {
		return nil, err
	}
	if err := syscall.Flock(int(f.Fd()), syscall.LOCK_EX); err != nil {
		f.Close()
		return nil, err
	}
	return func() { f.Close() }, nil
}

// Login is one Claude Code account rush can sign ~/.claude in as. Every
// session shares ~/.claude (settings, transcripts, history); what differs
// between accounts is only the sign-in, which rush keeps in its Vault and
// puts in place when you switch.
type Login struct {
	Name  string `json:"name"`
	ID    string `json:"id"` // the account's uuid
	Email string `json:"email,omitempty"`
	Org   string `json:"org,omitempty"`
	// Profile is the oauthAccount block Claude Code keeps beside the
	// sign-in in its state file: who the account is, not a secret.
	Profile jsontext.Value `json:"profile,omitzero"`
}

// UsageKey is where the login's usage readings are kept.
func (l Login) UsageKey() string { return "login:" + l.ID }

// Folder is a Claude Code config folder an older rush was given.
type Folder struct {
	Name      string `json:"name"`
	ConfigDir string `json:"configDir"`
}

// Profile is the folder as the logins agent's profile.
func (f Folder) Profile() agent.Profile {
	return agent.Profile{Kind: agent.Kind(LoginsKind), Name: f.Name, Dir: f.ConfigDir}
}

// home is the logins agent's own home (~/.claude), as its adapter says.
func home() Folder {
	h, ok := agent.As[agent.Homer](agent.Kind(LoginsKind))
	if !ok {
		return Folder{}
	}
	p := h.Home()
	return Folder{Name: p.Name, ConfigDir: p.Dir}
}
