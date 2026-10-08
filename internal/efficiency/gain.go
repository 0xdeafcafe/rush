package efficiency

import (
	"context"
	"os/exec"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
)

// Gain is a day of a saver's own account of what it saved. It's the
// saver's estimate (rtk counts bytes of shell output over four), shown
// beside what rush measured, never instead of it.
type Gain struct {
	Day      string  `json:"date"`
	Commands int     `json:"commands"`
	Saved    int64   `json:"saved_tokens"`
	Pct      float64 `json:"savings_pct"`
}

// RTKGains is rtk's daily record, from `rtk gain -d -f json`.
func RTKGains() ([]Gain, error) {
	bin := LookPath("rtk")
	if bin == "" {
		return nil, nil
	}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	out, err := exec.CommandContext(ctx, bin, "gain", "-d", "-f", "json").Output()
	if err != nil {
		return nil, err
	}
	var r struct {
		Daily []Gain `json:"daily"`
	}
	if err := jsonx.Unmarshal(out, &r); err != nil {
		return nil, err
	}
	return r.Daily, nil
}
