package plugin

import (
	"context"
	"encoding/json/jsontext"
	"fmt"
	"regexp"
	"slices"
	"sort"
	"sync"
	"time"

	"github.com/0xdeafcafe/photon/uithread"
)

// --- Services: what one plugin answers for others ---

// A plugin lists in provides the services it answers, and in uses the
// ones it may ask for. Asking for a service (services.ask, to the broker)
// calls service.call on every running plugin that provides it, at once,
// and hands back each answer by plugin. So a plugin can feed another
// without either knowing the other's name: Haven provides cleanup.report,
// and the cleanup plugin (with rush's own cleanup agent) uses whatever
// answers it.
//
// The broker answers rush itself for any service; a plugin only for the
// ones in its uses.

// Services rush knows the shape of.
const (
	// ServiceCleanupReport is what a plugin knows about freeing disk and
	// memory: a CleanupReport.
	ServiceCleanupReport = "cleanup.report"
)

// CleanupReport is a cleanup.report answer.
type CleanupReport struct {
	// Title names its source in the cleanup agent's task: "Haven".
	Title string `json:"title"`
	// Report is what it found, as text for the agent.
	Report string `json:"report"`
	// Rules is what the agent may do with it, and what it must never do.
	Rules string `json:"rules,omitempty"`
}

// ServiceTimeout is how long one plugin has to answer a service.call.
const ServiceTimeout = 45 * time.Second

// Answer is one plugin's answer to a services.ask.
type Answer struct {
	Plugin string         `json:"plugin"`
	Result jsontext.Value `json:"result,omitzero"`
	Error  string         `json:"error,omitempty"`
}

var serviceRE = sync.OnceValue(func() *regexp.Regexp {
	return regexp.MustCompile(`^[a-z][a-z0-9-]*(\.[a-z][a-z0-9-]*)+$`)
})

func (m *Manifest) validateServices() error {
	for _, l := range [][]string{m.Provides, m.Uses} {
		for _, s := range l {
			if !serviceRE().MatchString(s) {
				return fmt.Errorf("service %q: use dotted lowercase names, like cleanup.report", s)
			}
		}
	}
	if len(m.Provides) > 0 && m.Proto() != ProtoRush {
		return fmt.Errorf("an MCP plugin cannot provide services: it has no way to answer them")
	}
	return nil
}

// AskService asks every running plugin that provides service, through the
// broker, and returns their answers. None is not an error: nobody
// provides it, or the broker isn't running.
func AskService(ctx context.Context, service string) ([]Answer, error) {
	c, err := DialBroker()
	if err != nil {
		return nil, nil
	}
	defer c.Close()
	var out []Answer
	err = c.Call(ctx, "services.ask", map[string]string{"service": service}, &out)
	return out, err
}

// Providers names the plugins that run and provide service. It reads the
// disk, so it's not for the UI goroutine.
func Providers(service string) []string {
	uithread.Forbid("plugin.Providers")
	var out []string
	for name, a := range Enabled() {
		if slices.Contains(a.Manifest.Provides, service) && a.Manifest.Unmet() == "" {
			out = append(out, name)
		}
	}
	sort.Strings(out)
	return out
}
