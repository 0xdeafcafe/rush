package claude

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/0xdeafcafe/rush/internal/state"
)

// homesDir is where rush keeps each Claude login's home (LinkHome).
func homesDir() string { return filepath.Join(state.Dir(), string(Kind)) }

// HomeOf is the config folder rush keeps for the login with id.
func HomeOf(id string) Account {
	return Account{ConfigDir: filepath.Join(homesDir(), id)}
}

// Active is ~/.claude, where every session runs, signed in as whichever
// login is in use, named as cfg names it.
func Active(cfg state.Config) Account { //nolint:gocritic // Config goes by value, as everywhere in state
	root := DefaultAccount()
	for _, f := range cfg.Folders {
		if f.ConfigDir == root.ConfigDir && f.Name != "" {
			root.Name = f.Name
		}
	}
	return root
}

// Using is the login new Claude Code sessions run as, in its home;
// "" runs them in ~/.claude as whoever it's signed in as. It's a file of
// its own, written as the switch is made, so a session that rests for a
// switch finds it when it starts again.
func Using() string {
	b, err := os.ReadFile(filepath.Join(homesDir(), "using"))
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(b))
}

// UsingShown is Using for showing, not for starting a session: the fleet
// asks it every reading.
// shortcut: another rush process's switch shows up to usingFor late; a
// watch on homesDir if that ever matters.
func UsingShown() string {
	usingMemo.Lock()
	defer usingMemo.Unlock()
	if dir := homesDir(); usingMemo.dir == dir && time.Since(usingMemo.at) < usingFor {
		return usingMemo.id
	}
	usingMemo.dir, usingMemo.id, usingMemo.at = homesDir(), Using(), time.Now()
	return usingMemo.id
}

// usingMemo is UsingShown's last answer.
var usingMemo struct {
	sync.Mutex
	dir, id string
	at      time.Time
}

const usingFor = 5 * time.Second

// forgetUsing makes the next UsingShown read the disk.
func forgetUsing() {
	usingMemo.Lock()
	usingMemo.at = time.Time{}
	usingMemo.Unlock()
}

// SetUsing makes id the login new sessions run as; "" is ~/.claude.
func SetUsing(id string) error {
	defer forgetUsing()
	path := filepath.Join(homesDir(), "using")
	if id == "" {
		if err := os.Remove(path); err != nil && !os.IsNotExist(err) {
			return err
		}
		return nil
	}
	if err := os.MkdirAll(homesDir(), 0o700); err != nil {
		return err
	}
	tmp := path + ".tmp"
	if err := os.WriteFile(tmp, []byte(id+"\n"), 0o600); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}

// RunAccount is the config folder new Claude Code sessions run in: the
// home of the login in use, or ~/.claude. It reads the disk.
func RunAccount(c state.Config) Account { //nolint:gocritic // Config goes by value, as everywhere in state
	root := Active(c)
	id := Using()
	l, ok := c.Login(id)
	if id == "" || !ok {
		return root
	}
	h := HomeOf(id)
	h.Name = l.Name
	return h
}

// UseLogin readies l's home and makes it the one new sessions run in; the
// login ~/.claude is signed in as runs there instead, so its sign-in is
// never in two places.
func UseLogin(root Account, l Login) error {
	if SignedInAs(root) == l.ID {
		return SetUsing("")
	}
	h := HomeOf(l.ID)
	if err := LinkHome(h, root); err != nil {
		return err
	}
	if !HasHome(h) {
		cred, err := state.Vault().Get(l.ID)
		if err != nil {
			return fmt.Errorf("rush has no sign-in for %s; sign in to it again", l.Name)
		}
		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()
		if id, err := Owner(ctx, cred); err == nil && id != l.ID {
			return fmt.Errorf("rush's sign-in for %s is another account's; sign in to it again", l.Name)
		}
		if err := SeedHome(h, root, l, cred); err != nil {
			return err
		}
	} else if err := SeedHome(h, root, l, nil); err != nil {
		return err
	}
	return SetUsing(l.ID)
}

// AdoptLogin keeps the sign-in just made in scratch as its login's, in
// the vault and, when the login has a home already, there too: signing in
// to it again.
func AdoptLogin(scratch Account) (Login, error) {
	l, err := TheVault().Adopt(scratch)
	if err != nil {
		return l, err
	}
	if cred, err := state.Vault().Get(l.ID); err == nil {
		_ = SignHome(HomeOf(l.ID), l, cred)
	}
	return l, nil
}

// ForgetLogin drops what rush keeps of a login: its vault sign-in and
// its home. Sessions it's the login in use for go back to ~/.claude.
func ForgetLogin(id string) error {
	if Using() == id {
		_ = SetUsing("")
	}
	_ = RemoveHome(HomeOf(id))
	return state.Vault().Forget(id)
}
