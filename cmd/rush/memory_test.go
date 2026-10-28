package main

import (
	"strings"
	"testing"
)

func TestMemoryAddFlags(t *testing.T) {
	n, words, err := addFlags([]string{"--type=user", "likes", "--name", "tabs", "--", "--weird", "text"})
	if err != nil || n.Type != "user" || n.Name != "tabs" || strings.Join(words, " ") != "likes --weird text" {
		t.Fatalf("%+v %v %v", n, words, err)
	}
	if _, _, err := addFlags([]string{"x", "--type"}); err == nil {
		t.Fatal("a flag with no value is an error")
	}
	if _, _, err := addFlags([]string{"--type", "bogus", "x"}); err == nil {
		t.Fatal("an unknown type is an error")
	}
}
