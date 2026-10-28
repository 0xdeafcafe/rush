package plugin

import (
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"

	"github.com/0xdeafcafe/rush/internal/selfexe"
	"github.com/0xdeafcafe/rush/internal/state"
)

// Launch is how to start one plugin.
type Launch struct {
	Plugin Plugin
	// ProxyPort is rush's proxy for this plugin, or 0 for no network.
	ProxyPort int
}

// Env is the whole environment a plugin gets: nothing of yours (no tokens,
// no Claude config), only where it may write, how to reach the proxy, and
// what its manifest sets.
func (l Launch) Env() []string {
	m := l.Plugin.Manifest
	data := DataDir(m.Name)
	env := map[string]string{}
	expand := strings.NewReplacer("${DATA}", data, "${PLUGIN}", l.Plugin.Dir)
	for k, v := range m.Env {
		// ~ is yours, not the plugin's HOME: how it learns where another
		// tool's files, given it in read or write, are.
		env[k] = expandHome(expand.Replace(v))
	}
	for _, k := range []string{"LANG", "LC_ALL", "TZ"} {
		if v := os.Getenv(k); v != "" {
			env[k] = v
		}
	}
	// Set last, so a manifest can't move them.
	tmp := filepath.Join(data, "tmp")
	fixed := map[string]string{
		"PATH": "/usr/bin:/bin", "HOME": data, "TMPDIR": tmp + "/",
		"RUSH_PLUGIN": m.Name, "RUSH_PLUGIN_DATA": data,
		// and as they were called before the rename, for plugins written then
		"AGTOP_PLUGIN": m.Name, "AGTOP_PLUGIN_DATA": data,
	}
	if m.Proto() == ProtoRush {
		fixed["RUSH_IPC_FD"] = "3"
		fixed["AGTOP_IPC_FD"] = "3"
	}
	if l.ProxyPort > 0 {
		proxy := "http://127.0.0.1:" + strconv.Itoa(l.ProxyPort)
		for _, k := range []string{"HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"} {
			fixed[k] = proxy
		}
		// Node reads the proxy variables only when asked to.
		fixed["NODE_USE_ENV_PROXY"] = "1"
		fixed["NO_PROXY"], fixed["no_proxy"] = "", ""
	}
	for k, v := range fixed {
		env[k] = v
	}
	out := make([]string, 0, len(env))
	for k, v := range env {
		out = append(out, k+"="+v)
	}
	return out
}

// Process is the plugin's process, its stdio and extra files for the caller
// to wire up: a bundled plugin is rush itself, `rush plugin run <name>`,
// in its data folder; any other is Command, sandboxed.
func (l Launch) Process() (*exec.Cmd, error) {
	if !l.Plugin.Bundled {
		return l.Command()
	}
	exe, err := selfexe.Path()
	if err != nil {
		return nil, err
	}
	data := DataDir(l.Plugin.Name)
	if err := os.MkdirAll(filepath.Join(data, "tmp"), 0o700); err != nil {
		return nil, err
	}
	cmd := exec.Command(exe, "plugin", "run", l.Plugin.Name)
	cmd.Dir = data
	// Its HOME is its data folder, so it's told where rush's own state is:
	// without it, the gate plugin read no settings and ran no guard.
	cmd.Env = append(l.Env(), "RUSH_HOME="+state.Dir())
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	return cmd, nil
}

// Program is the path of the plugin's program, symlinks resolved.
func (l Launch) Program() (string, error) {
	p := l.Plugin.Command[0]
	if !filepath.IsAbs(p) {
		p = filepath.Join(l.Plugin.Dir, p)
	}
	return filepath.EvalSymlinks(p)
}

func resolved(p string) string {
	if r, err := filepath.EvalSymlinks(p); err == nil {
		return r
	}
	return filepath.Clean(p)
}

var errNoSandbox = errors.New("plugins need a sandbox, and there isn't one on this system yet (macOS only for now)")
