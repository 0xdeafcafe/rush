package host

import (
	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
)

// config.json has always kept the session's profile as {"name",
// "configDir"}, and older rushes still read and write it in that shape, so
// Account goes on disk and on the wire as it always has.
type wireAccount struct {
	Name      string `json:"name"`
	ConfigDir string `json:"configDir"`
}

type plainConfig Config

// wireConfig is a Config as config.json has it: its own Account field is
// shallower than plainConfig's, so it's the one read and written.
type wireConfig struct {
	plainConfig
	Account wireAccount `json:"account"`
}

func (cfg Config) MarshalJSON() ([]byte, error) { //nolint:gocritic // a value receiver, so a Config is written this way whether or not it is addressable
	return jsonx.Marshal(wireConfig{plainConfig(cfg), wireAccount{Name: cfg.Account.Name, ConfigDir: cfg.Account.Dir}})
}

func (cfg *Config) UnmarshalJSON(b []byte) error {
	var w wireConfig
	if err := jsonx.Unmarshal(b, &w); err != nil {
		return err
	}
	*cfg = Config(w.plainConfig)
	cfg.Kind = string(agent.Migrated(cfg.Kind)) // migration: an older rush's config names no kind for Claude Code
	cfg.Account = agent.Profile{Kind: agent.Kind(cfg.Kind), Name: w.Account.Name, Dir: w.Account.ConfigDir}
	return nil
}
