package gitinfo

import (
	"fmt"
	"reflect"
	"strings"
	"testing"
)

func TestParsePorcelainV2Status(t *testing.T) {
	input := []byte(
		"# branch.oid abcdef0123456789\x00" +
			"# branch.head feature/workspace\x00" +
			"# branch.upstream origin/feature/workspace\x00" +
			"# branch.ab +3 -2\x00" +
			"1 M. N... 100644 100644 100644 a b staged.go\x00" +
			"1 .M N... 100644 100644 100644 a b unstaged.go\x00" +
			"2 MM N... 100644 100644 100644 a b R100 renamed.go\x00old.go\x00" +
			"u UU N... 100644 100644 100644 100644 a b c conflict.go\x00" +
			"? new.go\x00" +
			"! generated.bin\x00",
	)
	status, err := parsePorcelainV2Status(input)
	if err != nil {
		t.Fatalf("parsePorcelainV2Status: %v", err)
	}
	if status.OID != "abcdef0123456789" || status.Branch != "feature/workspace" ||
		status.Upstream != "origin/feature/workspace" || status.Ahead != 3 || status.Behind != 2 {
		t.Fatalf("branch status = %#v", status)
	}
	if status.Staged != 2 || status.Unstaged != 2 || status.Conflicted != 1 ||
		status.Untracked != 1 || status.Ignored != 1 {
		t.Fatalf("file counts = %#v", status)
	}
	if !reflect.DeepEqual(status.ChangedPaths, []string{
		"conflict.go", "old.go", "renamed.go", "staged.go", "unstaged.go",
	}) || !reflect.DeepEqual(status.UntrackedPaths, []string{"new.go"}) {
		t.Fatalf("status paths = changed %#v untracked %#v", status.ChangedPaths, status.UntrackedPaths)
	}
}

func TestParsePorcelainV2DetachedAndInitial(t *testing.T) {
	for _, test := range []struct {
		name     string
		head     string
		detached bool
		initial  bool
	}{
		{name: "detached", head: "(detached)", detached: true},
		{name: "initial", head: "(initial)", initial: true},
	} {
		t.Run(test.name, func(t *testing.T) {
			status, err := parsePorcelainV2Status([]byte(
				"# branch.oid (initial)\x00# branch.head " + test.head + "\x00",
			))
			if err != nil {
				t.Fatal(err)
			}
			if status.Detached != test.detached || status.Initial != test.initial {
				t.Fatalf("status = %#v", status)
			}
		})
	}
}

func TestParsePorcelainV2StatusRejectsEscapingPathsAndBoundsOutput(t *testing.T) {
	for _, path := range []string{"../secret", "/absolute", "nested/../../secret"} {
		if _, err := parsePorcelainV2Status([]byte("? " + path + "\x00")); err == nil {
			t.Fatalf("accepted escaping status path %q", path)
		}
	}
	var input strings.Builder
	for index := 0; index < maxStatusPaths+3; index++ {
		input.WriteString(fmt.Sprintf("? path-%04d\x00", index))
	}
	status, err := parsePorcelainV2Status([]byte(input.String()))
	if err != nil {
		t.Fatal(err)
	}
	if len(status.UntrackedPaths) != maxStatusPaths ||
		status.UntrackedPathBounds.Total != maxStatusPaths+3 ||
		status.UntrackedPathBounds.More != 3 {
		t.Fatalf("untracked bounds = %#v len=%d", status.UntrackedPathBounds, len(status.UntrackedPaths))
	}
}

func TestParsePorcelainV2RejectsMalformedRecords(t *testing.T) {
	for _, input := range [][]byte{
		[]byte("# branch.ab ahead behind\x00"),
		[]byte("1 X\x00"),
		[]byte("2 R. too-short\x00"),
		[]byte("2 R. N... 1 1 1 a b R100 new\x00"),
		[]byte("unexpected\x00"),
	} {
		if _, err := parsePorcelainV2Status(input); err == nil {
			t.Fatalf("accepted malformed input %q", input)
		}
	}
}
