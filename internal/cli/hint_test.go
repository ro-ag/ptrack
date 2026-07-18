package cli

import (
	"strings"
	"testing"
)

func TestNoProjectHint(t *testing.T) {
	h := NoProjectHint()
	for _, w := range []string{"P-TRACK", "GET STARTED", "ptrack init", "--goal", "--help", "dashboard"} {
		if !strings.Contains(h, w) {
			t.Errorf("hint missing %q:\n%s", w, h)
		}
	}
}
