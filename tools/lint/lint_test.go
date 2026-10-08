package lint

import (
	"testing"

	"golang.org/x/tools/go/analysis/analysistest"
)

func TestJSON(t *testing.T) {
	analysistest.Run(t, analysistest.TestData(), JSON, "j", "example.com/photon/jsonx")
}

func TestUIBlock(t *testing.T) {
	analysistest.Run(t, analysistest.TestData(), UIBlock, "example.com/internal/ui")
}

func TestHot(t *testing.T) { analysistest.Run(t, analysistest.TestData(), Hot, "hot") }

func TestRead(t *testing.T) { analysistest.Run(t, analysistest.TestData(), Read, "read") }

func TestHooks(t *testing.T) {
	analysistest.Run(t, analysistest.TestData(), Hooks, "example.com/internal/ui/hookcheck")
}

func TestAdapters(t *testing.T) {
	analysistest.Run(t, analysistest.TestData(), Adapters, "example.com/internal/core", "example.com/internal/adapters/other")
}
