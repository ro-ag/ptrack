// Package guide installs and renders the ptrack "agent guide" — a short block of
// instructions, written into a project's agent-instruction files (AGENTS.md,
// CLAUDE.md), that teaches an AI agent how to use ptrack to persist and reload
// session context. The block is marker-delimited so it can be refreshed
// idempotently without disturbing surrounding content.
package guide

import (
	"os"
	"path/filepath"
	"regexp"
	"strings"
)

const (
	beginMarker = "<!-- ptrack:begin -->"
	endMarker   = "<!-- ptrack:end -->"
)

// blockRe matches a complete marker-delimited block (and a trailing newline),
// non-greedily so multiple blocks are matched individually.
var blockRe = regexp.MustCompile(`(?s)` + regexp.QuoteMeta(beginMarker) + `.*?` + regexp.QuoteMeta(endMarker) + `\n?`)

// DefaultFiles are the agent-instruction files ptrack writes its guide into.
var DefaultFiles = []string{"AGENTS.md", "CLAUDE.md"}

// TemplateName is the file, under the global ptrack home, whose contents are
// appended to the installed guide as the user's own working agreements.
const TemplateName = "guide.md"

// Body returns the guide content (without the markers) — also what `guide
// --print` emits.
func Body() string {
	return `## ptrack — session context

This project uses ` + "`ptrack`" + ` to persist planning state so a fresh agent can
resume after a previous session grew too large.

**At session start** — reload context:
- ` + "`ptrack context`" + ` — goal, summary, active plan, open tasks, blockers, open issues, inventory (add ` + "`--json`" + ` to parse).

**If the project is empty** — populate it from this repo (README, docs, code, git
log, open issues), then keep it current:
- Goal: ` + "`ptrack goal set \"north star\"`" + `
- Milestones (checkpoints): ` + "`ptrack milestone add \"v1.0\" [--due YYYY-MM-DD]`" + `
- Plans (workstreams): ` + "`ptrack plan add \"...\" [--milestone N]`" + `, then ` + "`ptrack plan use N`" + `
- Tasks with status: ` + "`ptrack task add \"...\" [--plan N]`" + ` then ` + "`task start`" + ` (in progress) / ` + "`task done`" + ` / ` + "`task block`" + ` (todo = pending)
- Issues (bugs/problems): ` + "`ptrack issue add \"...\" [--severity high] [--task N]`" + `
- Decisions: ` + "`ptrack note add \"...\" [--task N | --plan N]`" + `

**Titles are names, not status.** Do not prefix titles with "Pending:", "In
progress:", "Done:", etc. — ptrack tracks status separately. Set it with
` + "`task start|done|block`" + `, ` + "`plan done|use`" + `, ` + "`milestone done`" + `, ` + "`issue close`" + `. Rename with
` + "`ptrack <plan|task|milestone|issue> rename <id> \"new title\"`" + `.

**Record decisions, not narration.** Notes are the human-visible audit trail of
what you did and *why*. When you make a choice, hit a blocker, or find a
constraint, capture it — one decision per note:
` + "`ptrack note add \"chose X over Y because Z\" --task N`" + `. Do not log routine
steps, tool output, or restate the code.

**Commits are tracked.** Reference the task in commit messages as ` + "`#<id>`" + ` so the
commit links to it (` + "`ptrack hook install`" + ` records commits automatically; each
commit's ` + "`#<id>`" + ` links it to that task, otherwise the active plan).

**Before ending** — save the narrative for the next agent:
- ` + "`ptrack summary set \"where we are\"`" + `

**Query on demand** (all bounded, ` + "`--json`" + ` available):
- ` + "`ptrack next`" + ` · ` + "`ptrack board`" + ` · ` + "`ptrack milestone list`" + ` · ` + "`ptrack plan show <id>`" + ` · ` + "`ptrack task show <id>`" + ` · ` + "`ptrack task list --status doing,blocked`" + ` · ` + "`ptrack issue list`" + ` · ` + "`ptrack search <term>`" + ` · ` + "`ptrack note list`" + `

If no project exists yet: ` + "`ptrack init --goal \"...\"`" + `.
`
}

// Rendered returns the guide body plus, when extra is non-empty, the user's
// global guidelines appended after a separator. This is the content between the
// markers, and also what `guide --print` emits.
func Rendered(extra string) string {
	extra = strings.TrimSpace(extra)
	if extra == "" {
		return Body()
	}
	return Body() + "\n---\n\n" + extra + "\n"
}

// Block returns the full marker-delimited section to embed in a file, including
// any extra global guidelines.
func Block(extra string) string {
	return beginMarker + "\n" + Rendered(extra) + endMarker + "\n"
}

// Install writes (or refreshes) the guide block into each named file under dir,
// creating files that don't exist. extra holds optional global guidelines to
// append inside the block. It returns the paths it wrote; a file is only
// rewritten when its guide block is missing or out of date, so repeated installs
// are idempotent and no-ops report an empty slice.
func Install(dir string, files []string, extra string) ([]string, error) {
	block := Block(extra)
	var written []string
	for _, name := range files {
		path := filepath.Join(dir, name)
		changed, err := upsertFile(path, block)
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
func upsertFile(path, block string) (bool, error) {
	existing, err := os.ReadFile(path)
	if err != nil && !os.IsNotExist(err) {
		return false, err
	}
	updated, changed := upsert(string(existing), block)
	if !changed {
		return false, nil
	}
	if err := os.WriteFile(path, []byte(updated), 0o644); err != nil {
		return false, err
	}
	return true, nil
}

// upsert returns content with the given guide block inserted or replaced, and
// whether the result differs from the input. It is robust to malformed marker
// state: a single well-formed block is replaced in place (preserving its
// position and surrounding docs); any duplicate blocks or orphaned markers are
// normalized away so the result always contains exactly one block, and no
// non-marker text is ever removed.
func upsert(content, block string) (string, bool) {
	begin := strings.Index(content, beginMarker)
	end := strings.Index(content, endMarker)

	// Fast path — exactly one well-formed block and no stray markers elsewhere:
	// replace in place to keep the block where the author put it.
	if begin >= 0 && end > begin {
		before, after := content[:begin], content[end+len(endMarker):]
		if !hasMarker(before) && !hasMarker(after) {
			after = strings.TrimPrefix(after, "\n")
			newContent := before + block + after
			return newContent, newContent != content
		}
	}

	// Malformed (orphaned or duplicate markers) or absent: strip every complete
	// block and any leftover orphan marker lines, then append one clean block.
	stripped := blockRe.ReplaceAllString(content, "")
	stripped = strings.ReplaceAll(stripped, beginMarker, "")
	stripped = strings.ReplaceAll(stripped, endMarker, "")
	base := strings.TrimRight(stripped, " \t\n")

	newContent := block
	if base != "" {
		newContent = base + "\n\n" + block
	}
	return newContent, newContent != content
}

// hasMarker reports whether s contains either guide marker.
func hasMarker(s string) bool {
	return strings.Contains(s, beginMarker) || strings.Contains(s, endMarker)
}
