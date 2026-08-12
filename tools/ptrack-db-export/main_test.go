package main

import (
	"bytes"
	"path/filepath"
	"strings"
	"testing"
)

func TestRunRejectsImplicitOrAmbiguousInputs(t *testing.T) {
	absoluteHome := filepath.Join(t.TempDir(), "home")
	absoluteOutput := filepath.Join(t.TempDir(), "stage")
	tests := []struct {
		name string
		args []string
		code int
		want string
	}{
		{name: "missing", code: 2, want: "--home and --output are required"},
		{name: "positional", args: []string{"--home", absoluteHome, "--output", absoluteOutput, "extra"}, code: 2, want: "positional arguments are not accepted"},
		{name: "relative home", args: []string{"--home", "legacy-home", "--output", absoluteOutput}, code: 1, want: "absolute and clean"},
		{name: "relative output", args: []string{"--home", absoluteHome, "--output", "stage"}, code: 1, want: "absolute and clean"},
		{name: "unknown flag", args: []string{"--unknown"}, code: 2, want: "flag provided but not defined"},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			var stderr bytes.Buffer
			if got := run(test.args, &stderr); got != test.code {
				t.Fatalf("run code = %d, want %d; stderr = %q", got, test.code, stderr.String())
			}
			if !strings.Contains(stderr.String(), test.want) {
				t.Fatalf("stderr = %q, want substring %q", stderr.String(), test.want)
			}
		})
	}
}
