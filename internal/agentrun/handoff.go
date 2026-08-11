package agentrun

import (
	"fmt"
	"path/filepath"
	"strings"
	"unicode/utf8"
)

const (
	maxHandoffEvents = 8
	maxHandoffBytes  = 2 * 1024
)

// HandoffPreview is deterministic, bounded, and read-only. Included event IDs
// provide provenance without exposing raw provider content.
type HandoffPreview struct {
	Text             string   `json:"text"`
	IncludedEventIDs []string `json:"includedEventIds"`
	ConsideredEvents int      `json:"consideredEvents"`
	Truncated        bool     `json:"truncated"`
}

// BuildHandoffPreview composes only current-association structured evidence
// and redacted agent-provided summaries. It never persists the result or
// changes run/task/project state.
func BuildHandoffPreview(run Run, events []Event) HandoffPreview {
	relevant := currentHandoffEvents(run, events)
	intelligence := DeriveRunIntelligence(run, relevant)
	lines := []string{fmt.Sprintf(
		"Agent run state: %s (%s confidence).",
		intelligence.State,
		confidenceOrLow(intelligence.Confidence),
	)}
	if association := run.Association; association != nil {
		switch {
		case association.Target.TaskID != 0:
			lines = append(lines, fmt.Sprintf(
				"Context: plan #%d, task #%d.",
				association.Target.PlanID,
				association.Target.TaskID,
			))
		case association.Target.PlanID != 0:
			lines = append(lines, fmt.Sprintf("Context: plan #%d.", association.Target.PlanID))
		default:
			lines = append(lines, "Context: project.")
		}
	} else {
		lines = append(lines, "Context: project (no current plan or task association).")
	}

	preview := HandoffPreview{
		IncludedEventIDs: []string{},
		ConsideredEvents: len(relevant),
	}
	seenLines := map[string]bool{}
	for index := len(relevant) - 1; index >= 0; index-- {
		line := handoffEventLine(relevant[index])
		if line == "" || seenLines[line] {
			continue
		}
		if len(preview.IncludedEventIDs) == maxHandoffEvents {
			preview.Truncated = true
			break
		}
		seenLines[line] = true
		lines = append(lines, "- "+line)
		preview.IncludedEventIDs = append(preview.IncludedEventIDs, relevant[index].ID)
	}
	if len(preview.IncludedEventIDs) == 0 {
		lines = append(lines, "No retained structured work-product events for the current context.")
	}
	preview.Text, preview.Truncated = boundedHandoffText(
		strings.Join(lines, "\n"),
		preview.Truncated,
	)
	return preview
}

func currentHandoffEvents(run Run, events []Event) []Event {
	ordered := currentRunEvents(run, events)
	current := run.Association
	relevant := make([]Event, 0, len(ordered))
	for _, event := range ordered {
		correlation := event.Correlation
		if correlation.ProjectRoot != run.ProjectRoot || correlation.TerminalID != run.TerminalID {
			continue
		}
		if current == nil {
			if correlation.PlanID == 0 && correlation.TaskID == 0 &&
				correlation.Generation == 0 && correlation.AssociationRevision == 0 {
				relevant = append(relevant, event)
			}
			continue
		}
		if correlation.PlanID == current.Target.PlanID &&
			correlation.TaskID == current.Target.TaskID &&
			correlation.Generation == current.Generation &&
			correlation.AssociationRevision == current.Revision {
			relevant = append(relevant, event)
		}
	}
	return relevant
}

func handoffEventLine(event Event) string {
	phase := string(event.Phase)
	subject := safeHandoffScalar(event.Subject)
	switch event.Kind {
	case EventLifecycle:
		return "Lifecycle " + phase + "."
	case EventTool:
		return handoffSubjectLine("Tool", phase, subject)
	case EventCommand:
		return handoffSubjectLine("Command", phase, subject)
	case EventFile:
		paths := safeHandoffPaths(event.Paths)
		if paths == "" {
			return "File activity " + phase + "."
		}
		return "File activity " + phase + ": " + paths + "."
	case EventTest:
		return handoffSubjectLine("Test", phase, subject)
	case EventCommit:
		sha := safeHandoffScalar(event.CommitSHA)
		if len(sha) > 12 {
			sha = sha[:12]
		}
		return handoffSubjectLine("Commit", phase, sha)
	case EventError:
		return handoffSubjectLine("Error", phase, safeHandoffScalar(event.ErrorClass))
	case EventSummary:
		if containsReasoningMarker(event.Summary) {
			return ""
		}
		summary := strings.Join(strings.Fields(event.Summary), " ")
		if highRiskCredential.MatchString(summary) || privateKeyMarker.MatchString(summary) {
			return ""
		}
		summary = redactEventSummary(summary)
		if !validEventText(summary, true) || summary == "" {
			return ""
		}
		return "Agent-provided summary: " + summary
	default:
		return ""
	}
}

func handoffSubjectLine(kind, phase, subject string) string {
	if subject == "" {
		return kind + " " + phase + "."
	}
	return kind + " " + phase + ": " + subject + "."
}

func safeHandoffScalar(value string) string {
	value = strings.TrimSpace(value)
	if value == "" || len(value) > maxEventPathBytes ||
		!validEventText(value, false) || containsCredentialLike(value) ||
		containsReasoningMarker(value) {
		return ""
	}
	return value
}

func safeHandoffPaths(paths []string) string {
	safe := make([]string, 0, min(len(paths), 3))
	for _, path := range paths {
		clean := filepath.Clean(filepath.FromSlash(path))
		if clean == "." || filepath.IsAbs(clean) || clean == ".." ||
			strings.HasPrefix(clean, ".."+string(filepath.Separator)) ||
			safeHandoffScalar(path) == "" {
			continue
		}
		safe = append(safe, filepath.ToSlash(clean))
		if len(safe) == 3 {
			break
		}
	}
	return strings.Join(safe, ", ")
}

func confidenceOrLow(confidence IntelligenceConfidence) IntelligenceConfidence {
	if confidence == "" {
		return ConfidenceLow
	}
	return confidence
}

func boundedHandoffText(value string, alreadyTruncated bool) (string, bool) {
	if len(value) <= maxHandoffBytes {
		return value, alreadyTruncated
	}
	value = value[:maxHandoffBytes-len("\n…")]
	for !utf8.ValidString(value) {
		value = value[:len(value)-1]
	}
	return strings.TrimSpace(value) + "\n…", true
}
