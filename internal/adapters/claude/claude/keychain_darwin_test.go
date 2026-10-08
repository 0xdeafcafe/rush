package claude

import (
	"bytes"
	"os"
	"testing"

	"github.com/0xdeafcafe/photon/keychain"
)

// Claude Code reads its sign-in under your user name. Another item under
// the same service (an older tool's) mustn't catch what a switch writes.
func TestCredsGoToClaudeCodesItem(t *testing.T) {
	if os.Getenv("RUSH_KEYCHAIN_TEST") == "" {
		t.Skip("writes to your login keychain; set RUSH_KEYCHAIN_TEST=1")
	}
	a := Account{ConfigDir: t.TempDir()}
	svc := a.keychainService()
	defer keychain.Delete(svc, "unknown")
	defer keychain.Delete(svc, keychain.User())
	if err := keychain.Write(svc, "unknown", []byte(`{"claudeAiOauth":{"refreshToken":"stale"}}`)); err != nil {
		t.Fatal(err)
	}
	want := []byte(`{"claudeAiOauth":{"refreshToken":"new"}}`)
	if err := writeCreds(a, want); err != nil {
		t.Fatal(err)
	}
	if got, err := keychain.Read(svc, keychain.User()); err != nil || !bytes.Equal(got, want) {
		t.Fatalf("Claude Code's item: %s, %v", got, err)
	}
	if got, err := readCreds(a); err != nil || !bytes.Equal(got, want) {
		t.Fatalf("read back: %s, %v", got, err)
	}
}
