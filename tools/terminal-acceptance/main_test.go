package main

import (
	"bytes"
	"errors"
	"strings"
	"testing"
)

func TestInventoryReportsOnlyBoundedAvailability(t *testing.T) {
	var output bytes.Buffer
	err := writeInventory(
		&output,
		func(name string) (string, error) {
			if name == "zsh" {
				return "/secret/location/zsh", nil
			}
			return "", errors.New("missing")
		},
		func(name string) string {
			if name == "TERM" {
				return "xterm-256color"
			}
			return ""
		},
	)
	if err != nil {
		t.Fatalf("writeInventory: %v", err)
	}
	result := output.String()
	if !strings.Contains(result, "term=set") ||
		!strings.Contains(result, "zsh=available") ||
		!strings.Contains(result, "codex=missing") {
		t.Fatalf("inventory = %q", result)
	}
	if strings.Contains(result, "/secret/") || strings.Contains(result, "xterm-256color") {
		t.Fatalf("inventory exposed environment or executable path: %q", result)
	}
}

func TestRenderFixtureIncludesUnicodeAndBoundedOSC8(t *testing.T) {
	var output bytes.Buffer
	if err := writeRenderFixture(&output); err != nil {
		t.Fatalf("writeRenderFixture: %v", err)
	}
	result := output.String()
	for _, expected := range []string{"cafe\u0301", "日本語", "🧑‍💻", "\x1b]8;;https://example.com/"} {
		if !strings.Contains(result, expected) {
			t.Fatalf("render fixture missing %q", expected)
		}
	}
}

func TestOutputFixtureIsExactAndBounded(t *testing.T) {
	var output bytes.Buffer
	if err := writeOutputFixture(&output, 1); err != nil {
		t.Fatalf("writeOutputFixture: %v", err)
	}
	if output.Len() != 1024*1024 {
		t.Fatalf("output bytes = %d, want %d", output.Len(), 1024*1024)
	}
	for _, invalid := range []int{0, maximumOutputMiB + 1} {
		if err := writeOutputFixture(&bytes.Buffer{}, invalid); err == nil {
			t.Fatalf("writeOutputFixture(%d) succeeded", invalid)
		}
	}
}

func TestRunRejectsUnknownAndInvalidOutputArguments(t *testing.T) {
	for _, arguments := range [][]string{
		{"unknown"},
		{"output", "--mib", "0"},
		{"output", "extra"},
		{"interactive", "extra"},
	} {
		if err := run(arguments, &bytes.Buffer{}, &bytes.Buffer{}); err == nil {
			t.Fatalf("run(%v) succeeded", arguments)
		}
	}
}
