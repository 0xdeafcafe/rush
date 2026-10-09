package ui

import (
	"fmt"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/host"
)

// A stopped session can be carried too; active or queued work must not be
// silently abandoned by a snapshot taken before it finishes.
func handoffQuiet(i host.Info) bool {
	return (i.State == "idle" || i.State == "stopped") && len(i.Queue) == 0 && i.Needs == "" && len(i.Background) == 0 && i.Retry == nil
}

// toldOf is whether c holds any of the conversation to carry on: none means
// telling it failed, and the source is the only place it still exists.
func toldOf(c agent.Conversation) bool {
	return c.First != "" || len(c.Steps) != 0 || len(c.Recent) != 0 || len(c.History) != 0
}

// The host clears Carry only after the adapter emits Init. Socket readiness
// alone does not establish that the destination accepted the history.
func waitCarried(id string) error {
	return awaitCarried(func() (bool, error) {
		info, err := host.ReadInfo(id)
		if err != nil {
			return false, err
		}
		if info.Error != "" {
			return false, fmt.Errorf("%s", info.Error)
		}
		if info.State == "stopped" {
			return false, fmt.Errorf("destination stopped")
		}
		cfg, err := host.ReadConfig(id)
		return cfg.Carry == nil, err
	}, 30*time.Second)
}

func awaitCarried(read func() (bool, error), timeout time.Duration) error {
	timer := time.NewTimer(timeout)
	defer timer.Stop()
	tick := time.NewTicker(50 * time.Millisecond)
	defer tick.Stop()
	for {
		ready, err := read()
		if err != nil || ready {
			return err
		}
		select {
		case <-timer.C:
			return fmt.Errorf("initialization timed out")
		case <-tick.C:
		}
	}
}
