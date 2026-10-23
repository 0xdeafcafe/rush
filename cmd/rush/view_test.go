package main

import (
	"testing"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/host"
)

// Each example is a view rush view set takes as it is.
func TestViewExamples(t *testing.T) {
	for name, ex := range viewExamples {
		var v host.View
		if err := jsonx.Unmarshal([]byte(ex), &v); err != nil || v.Title == "" {
			t.Errorf("%s: %v", name, err)
		}
	}
}
