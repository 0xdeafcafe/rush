package main

// The plugins that come with rush: see plugin.Bundle.
import (
	_ "github.com/0xdeafcafe/rush/internal/bundled/clean"
	_ "github.com/0xdeafcafe/rush/internal/bundled/cloudflared"
	_ "github.com/0xdeafcafe/rush/internal/bundled/drafts"
	_ "github.com/0xdeafcafe/rush/internal/bundled/gate"
	_ "github.com/0xdeafcafe/rush/internal/bundled/hexview"
	_ "github.com/0xdeafcafe/rush/internal/bundled/queue"
	_ "github.com/0xdeafcafe/rush/internal/bundled/tailscale"
)
