package menubar

import (
	"crypto/sha256"
	"embed"
	"encoding/hex"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"runtime"
	"strconv"
	"strings"
	"syscall"
	"time"

	"github.com/0xdeafcafe/rush/internal/events"
	"github.com/0xdeafcafe/rush/internal/state"
)

//go:embed app.swift
var source string

// icons are the app's icon, which its notifications carry: the bottle on
// its tile, and on a full square for macOS 26 on, which draws its own tile
// (docs/brand/finish-icon.sh makes both). The menu bar's bottle is drawn
// by app.swift.
//
//go:embed icons/*.icns
var icons embed.FS

// BundleID is the app's identity: macOS keeps its notification settings
// under it.
const BundleID = "dev.rush.menubar"

func dir() string      { return filepath.Join(state.Dir(), "menubar") }
func lockPath() string { return filepath.Join(dir(), "feed.lock") }

// AppPath is where the built app lives.
func AppPath() string { return filepath.Join(dir(), "rush.app") }

// hold marks the feed as running for as long as it runs, with the app's
// pid, so rush leaves notifications to it and can quit it.
func hold() func() {
	_ = os.MkdirAll(dir(), 0o700)
	f, err := os.OpenFile(lockPath(), os.O_RDWR|os.O_CREATE, 0o600)
	if err != nil {
		return func() {}
	}
	if syscall.Flock(int(f.Fd()), syscall.LOCK_EX|syscall.LOCK_NB) != nil {
		f.Close()
		return func() {}
	}
	_ = f.Truncate(0)
	_, _ = f.WriteAt([]byte(os.Getenv("RUSH_MENUBAR_PID")), 0)
	return func() {
		_ = f.Truncate(0)
		f.Close()
	}
}

// Running says whether the menu bar app is up, so rush's own
// notifications would only repeat its.
func Running() bool {
	f, err := os.Open(lockPath())
	if err != nil {
		return false
	}
	defer f.Close()
	if syscall.Flock(int(f.Fd()), syscall.LOCK_SH|syscall.LOCK_NB) != nil {
		return true
	}
	_ = syscall.Flock(int(f.Fd()), syscall.LOCK_UN)
	return false
}

// Start builds the app if rush changed since it was last built, and opens
// it; one already running is left alone unless it was just rebuilt. rushes
// starting together take turns, so only the first builds and opens it.
func Start() error {
	if runtime.GOOS != "darwin" {
		return errors.New("the menu bar icon is macOS only")
	}
	_ = os.MkdirAll(dir(), 0o700)
	if f, err := os.OpenFile(filepath.Join(dir(), "start.lock"), os.O_RDWR|os.O_CREATE, 0o600); err == nil {
		defer f.Close()
		_ = syscall.Flock(int(f.Fd()), syscall.LOCK_EX)
	}
	rebuilt, err := buildApp()
	if err != nil {
		events.Log("menubar-start", map[string]any{"err": err.Error()})
		return err
	}
	if rebuilt {
		Stop()
		for i := 0; i < 50 && Running(); i++ {
			time.Sleep(100 * time.Millisecond)
		}
	} else if Running() {
		return nil
	}
	err = exec.Command("/usr/bin/open", AppPath()).Run()
	f := map[string]any{"rebuilt": rebuilt}
	if err != nil {
		f["err"] = err.Error()
	}
	events.Log("menubar-start", f)
	return err
}

// Stop quits the app, every copy of it, and with it the feed.
func Stop() {
	events.Log("menubar-stop", nil)
	b, _ := os.ReadFile(lockPath())
	if pid, err := strconv.Atoi(strings.TrimSpace(string(b))); err == nil && pid > 1 && Running() {
		_ = syscall.Kill(pid, syscall.SIGTERM)
	}
	_ = exec.Command("/usr/bin/pkill", "-f", regexp.QuoteMeta(filepath.Join(AppPath(), "Contents", "MacOS", "rush-menubar"))).Run()
}

// build compiles the app with the Swift compiler that comes with Xcode's
// command line tools. It's rebuilt only when the source or the rush it
// runs changes; the stamp inside says which it was built from.
func buildApp() (rebuilt bool, err error) {
	app, bin := AppPath(), exe()
	h := sha256.New()
	full := tiles()
	h.Write([]byte(source + "\x00" + bin + "\x00" + strconv.FormatBool(full)))
	res, _ := icons.ReadDir("icons")
	for _, e := range res {
		b, _ := icons.ReadFile("icons/" + e.Name())
		h.Write(append([]byte("\x00"+e.Name()+"\x00"), b...))
	}
	stamp := hex.EncodeToString(h.Sum(nil)[:8])
	stampPath := app + ".stamp" // beside it: inside would break its signature
	if _, err := os.Stat(app); err == nil {
		if b, _ := os.ReadFile(stampPath); string(b) == stamp {
			return false, nil
		}
	}
	if _, err := exec.LookPath("swiftc"); err != nil {
		return false, errors.New("building the menu bar app needs the Swift compiler: xcode-select --install")
	}
	tmp, err := os.MkdirTemp("", "rush-menubar")
	if err != nil {
		return false, err
	}
	defer os.RemoveAll(tmp)
	src := filepath.Join(tmp, "main.swift")
	if err := os.WriteFile(src, []byte(source), 0o600); err != nil {
		return false, err
	}
	_ = os.RemoveAll(app)
	for _, d := range []string{"MacOS", "Resources"} {
		if err := os.MkdirAll(filepath.Join(app, "Contents", d), 0o755); err != nil {
			return false, err
		}
	}
	out, err := exec.Command("swiftc", "-O", "-swift-version", "5", "-parse-as-library", "-o", filepath.Join(app, "Contents", "MacOS", "rush-menubar"), src).CombinedOutput()
	if err != nil {
		return false, fmt.Errorf("building the menu bar app: %v\n%s", err, out)
	}
	for _, e := range res {
		name := e.Name()
		switch {
		case name == "AppIcon.icns" && full, name == "AppIcon-full.icns" && !full:
			continue
		case name == "AppIcon-full.icns":
			name = "AppIcon.icns"
		}
		b, _ := icons.ReadFile("icons/" + e.Name())
		if err := os.WriteFile(filepath.Join(app, "Contents", "Resources", name), b, 0o644); err != nil {
			return false, err
		}
	}
	if err := os.WriteFile(filepath.Join(app, "Contents", "Info.plist"), []byte(fmt.Sprintf(plist, BundleID, time.Now().Unix(), xmlEscape(bin))), 0o644); err != nil {
		return false, err
	}
	// Notifications need a signed app; an ad-hoc signature is enough on
	// the Mac that built it.
	if out, err := exec.Command("/usr/bin/codesign", "--force", "--sign", "-", app).CombinedOutput(); err != nil {
		return false, fmt.Errorf("signing the menu bar app: %v\n%s", err, out)
	}
	// Launch Services and Notification Center keep an app's icon by its
	// path and version; a new version, registered again, has them take
	// this build's (an older build may have had none).
	_ = exec.Command(lsregister, "-f", app).Run()
	return true, os.WriteFile(stampPath, []byte(stamp), 0o644)
}

// tiles says whether macOS draws app icons' tiles itself, as it does from
// 26 on; before, an icon brings its own.
func tiles() bool {
	out, _ := exec.Command("/usr/bin/sw_vers", "-productVersion").Output()
	major, _ := strconv.Atoi(strings.SplitN(strings.TrimSpace(string(out)), ".", 2)[0])
	return major >= 26
}

const lsregister = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"

// Forget stops the app opening at login.
func Forget() {
	home, _ := os.UserHomeDir()
	_ = os.Remove(filepath.Join(home, "Library", "LaunchAgents", BundleID+".plist"))
}

func xmlEscape(s string) string {
	return strings.NewReplacer("&", "&amp;", "<", "&lt;", ">", "&gt;").Replace(s)
}

const plist = `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleIdentifier</key><string>%s</string>
	<key>CFBundleName</key><string>rush</string>
	<key>CFBundleDisplayName</key><string>rush</string>
	<key>CFBundleExecutable</key><string>rush-menubar</string>
	<key>CFBundleIconFile</key><string>AppIcon</string>
	<key>CFBundlePackageType</key><string>APPL</string>
	<key>CFBundleShortVersionString</key><string>1.0</string>
	<key>CFBundleVersion</key><string>%d</string>
	<key>LSMinimumSystemVersion</key><string>14.0</string>
	<key>LSUIElement</key><true/>
	<key>RushBinary</key><string>%s</string>
</dict>
</plist>
`
