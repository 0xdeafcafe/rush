package harnessup

import (
	"context"
	"net/http"
	"net/url"
	"os"
	"os/exec"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
)

// latestOf is the newest version of a harness that can be known cheaply,
// or empty: Homebrew's when it installed it, else what the registry its
// package is in says.
func latestOf(ctx context.Context, pub agent.Published, s source) string {
	if v := brewLatest(ctx, s); v != "" {
		return v
	}
	if s.npm != "" {
		pub.NPM = s.npm
	}
	switch {
	case pub.NPM != "":
		var r struct {
			Version string `json:"version"`
		}
		if getJSON(ctx, "https://registry.npmjs.org/"+pub.NPM+"/latest", &r) == nil {
			return Parse(r.Version)
		}
	case pub.PyPI != "":
		var r struct {
			Info struct {
				Version string `json:"version"`
			} `json:"info"`
		}
		if getJSON(ctx, "https://pypi.org/pypi/"+url.PathEscape(pub.PyPI)+"/json", &r) == nil {
			return Parse(r.Info.Version)
		}
	}
	return ""
}

// brewLatest is the version Homebrew would install, from what it already
// knows (it doesn't update itself to say).
func brewLatest(ctx context.Context, s source) string {
	name, flag := s.brew, "--formula"
	if s.cask != "" {
		name, flag = s.cask, "--cask"
	}
	if name == "" {
		return ""
	}
	brew, ok := agent.Find("brew")
	if !ok {
		return ""
	}
	ctx, cancel := context.WithTimeout(ctx, 20*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, brew, "info", "--json=v2", flag, name)
	cmd.Env = append(os.Environ(), "HOMEBREW_NO_AUTO_UPDATE=1")
	out, err := cmd.Output()
	if err != nil {
		return ""
	}
	var r struct {
		Formulae []struct {
			Versions struct {
				Stable string `json:"stable"`
			} `json:"versions"`
		} `json:"formulae"`
		Casks []struct {
			Version string `json:"version"`
		} `json:"casks"`
	}
	if jsonx.Unmarshal(out, &r) != nil {
		return ""
	}
	switch {
	case len(r.Formulae) > 0:
		return Parse(r.Formulae[0].Versions.Stable)
	case len(r.Casks) > 0:
		return Parse(strings.SplitN(r.Casks[0].Version, ",", 2)[0])
	}
	return ""
}

func getJSON(ctx context.Context, u string, v any) error {
	ctx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, u, nil)
	if err != nil {
		return err
	}
	res, err := http.DefaultClient.Do(req)
	if err != nil {
		return err
	}
	defer res.Body.Close()
	if res.StatusCode != http.StatusOK {
		return &httpError{res.Status}
	}
	return jsonx.Decode(res.Body, v)
}

type httpError struct{ status string }

func (e *httpError) Error() string { return e.status }
