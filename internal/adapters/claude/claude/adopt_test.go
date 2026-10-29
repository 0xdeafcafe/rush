package claude

import (
	"bytes"
	"os"
	"path/filepath"
	"runtime"
	"testing"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/state"
)

func TestAdoptReplacesLoginWhereSessionsRun(t *testing.T) {
	if runtime.GOOS == "darwin" && os.Getenv("RUSH_KEYCHAIN_TEST") == "" {
		t.Skip("uses isolated temporary keychain items; set RUSH_KEYCHAIN_TEST=1")
	}
	for _, same := range []bool{true, false} {
		name := "another-account"
		if same {
			name = "same-account"
		}
		t.Run(name, func(t *testing.T) {
			dir := t.TempDir()
			t.Setenv("RUSH_HOME", filepath.Join(dir, "rush"))
			id := "rush-adopt-test-" + filepath.Base(filepath.Dir(dir))
			root := Account{ConfigDir: filepath.Join(dir, "root")}
			scratch := Account{ConfigDir: filepath.Join(dir, "signin")}
			home := HomeOf(id)
			for _, a := range []Account{root, scratch, home} {
				if err := os.MkdirAll(a.ConfigDir, 0700); err != nil {
					t.Fatal(err)
				}
				t.Cleanup(func() { _ = deleteCreds(a) })
			}
			t.Cleanup(func() { _ = state.Vault().Forget(id) })
			profile, err := jsonx.Marshal(map[string]string{"accountUuid": id})
			if err != nil {
				t.Fatal(err)
			}
			rootProfile := profile
			if !same {
				rootProfile = []byte(`{"accountUuid":"other"}`)
			}
			for _, a := range []Account{scratch, home} {
				if err := writeProfile(a.StatePath(), profile); err != nil {
					t.Fatal(err)
				}
			}
			if err := writeProfile(root.StatePath(), rootProfile); err != nil {
				t.Fatal(err)
			}
			stale := []byte(`{"claudeAiOauth":{"refreshToken":"stale"},"mcpOAuth":{"test":{"accessToken":"keep"}}}`)
			fresh := []byte(`{"claudeAiOauth":{"refreshToken":"fresh"}}`)
			for _, a := range []Account{root, home} {
				if err := writeCreds(a, stale); err != nil {
					t.Fatal(err)
				}
			}
			if err := writeCreds(scratch, fresh); err != nil {
				t.Fatal(err)
			}
			if _, err := adoptLogin(scratch, root); err != nil {
				t.Fatal(err)
			}
			want := stale
			if same {
				want = withMCPLogins(fresh, stale)
			}
			got, err := readCreds(root)
			if err != nil || !bytes.Equal(got, want) {
				t.Fatalf("root credential was not correctly updated: %v", err)
			}
			if SignedInAs(root) != map[bool]string{true: id, false: "other"}[same] {
				t.Fatal("changed root's account")
			}
			got, err = readCreds(home)
			want = fresh
			if same {
				want = withMCPLogins(fresh, stale)
			}
			if err != nil || !bytes.Equal(got, want) {
				t.Fatalf("home kept stale credential: %v", err)
			}
			if _, err := os.Stat(scratch.ConfigDir); !os.IsNotExist(err) {
				t.Fatal("scratch login was not removed")
			}
		})
	}
}
