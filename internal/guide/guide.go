// Package guide installs and renders the ptrack "agent guide" — a short block of
// instructions, written into a project's agent-instruction files (AGENTS.md,
// CLAUDE.md), that teaches an AI agent how to use ptrack to persist and reload
// session context. The block is marker-delimited so it can be refreshed
// idempotently without disturbing surrounding content.
package guide

import (
	"os"
	"path/filepath"
	"strings"
)

const (
	beginMarker = "<!-- ptrack:begin -->"
	endMarker   = "<!-- ptrack:end -->"
)

// DefaultFiles are the agent-instruction files ptrack writes its guide into.
var DefaultFiles = []string{"AGENTS.md", "CLAUDE.md"}

// Body returns the guide content (without the markers) — also what `guide
// --print` emits.
func Body() string {
	return `## ptrack — session context

This project uses ` + "`ptrack`" + ` to persist planning state so a fresh agent can
resume after a previous session grew too large.

**At session start** — reload context:
- ` + "`ptrack context`" + ` — goal, rolling summary, active plan, open tasks, blockers, recent decisions (add ` + "`--json`" + ` to parse).

**While working** — keep state current:
- Record decisions: ` + "`ptrack note add \"...\" [--task N | --plan N]`" + `
- Advance work: ` + "`ptrack task start|done|block <id>`" + `, ` + "`ptrack plan use|done <id>`" + `
- Add work: ` + "`ptrack plan add \"...\"`" + `, ` + "`ptrack task add \"...\" [--plan N]`" + `

**Before ending** — save the narrative for the next agent:
- ` + "`ptrack summary set \"where we are\"`" + `

**Query on demand** (all bounded, ` + "`--json`" + ` available):
- ` + "`ptrack next`" + ` · ` + "`ptrack board`" + ` · ` + "`ptrack plan show <id>`" + ` · ` + "`ptrack task show <id>`" + ` · ` + "`ptrack task list --status doing,blocked`" + ` · ` + "`ptrack search <term>`" + ` · ` + "`ptrack note list`" + `

If no project exists yet: ` + "`ptrack init --goal \"...\"`" + `.
`
}

// Block returns the full marker-delimited section to embed in a file.
func Block() string {
	return beginMarker + "\n" + Body() + endMarker + "\n"
}

// Install writes (or refreshes) the guide block into each named file under dir,
// creating files that don't exist. It returns the paths it wrote. A file is only
// rewritten when its guide block is missing or out of date, so repeated installs
// are idempotent and no-ops report an empty slice.
func Install(dir string, files []string) ([]string, error) {
	var written []string
	for _, name := range files {
		path := filepath.Join(dir, name)
		changed, err := upsertFile(path)
		if err != nil {
			return written, err
		}
		if changed {
			written = append(written, path)
		}
	}
	return written, nil
}

// upsertFile inserts or replaces the guide block in one file, returning whether
// the file changed.
func upsertFile(path string) (bool, error) {
	existing, err := os.ReadFile(path)
	if err != nil && !os.IsNotExist(err) {
		return false, err
	}
	updated, changed := upsert(string(existing))
	if !changed {
		return false, nil
	}
	if err := os.WriteFile(path, []byte(updated), 0o644); err != nil {
		return false, err
	}
	return true, nil
}

// upsert returns content with the guide block inserted or replaced, and whether
// the result differs from the input.
func upsert(content string) (string, bool) {
	block := Block()
	begin := strings.Index(content, beginMarker)
	end := strings.Index(content, endMarker)

	if begin >= 0 && end > begin {
		// Replace the existing block, including its markers. `block` already
		// ends with a newline, so drop a single leading newline from the tail to
		// avoid a blank-line gap.
		before := content[:begin]
		after := strings.TrimPrefix(content[end+len(endMarker):], "\n")
		newContent := before + block + after
		if newContent == content {
			return content, false
		}
		return newContent, true
	}

	// No block yet: append it, ensuring a blank line before.
	var b strings.Builder
	b.WriteString(content)
	if content != "" && !strings.HasSuffix(content, "\n\n") {
		if strings.HasSuffix(content, "\n") {
			b.WriteString("\n")
		} else {
			b.WriteString("\n\n")
		}
	}
	b.WriteString(block)
	return b.String(), true
}
