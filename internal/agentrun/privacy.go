package agentrun

import (
	"errors"
	"fmt"
	"net/url"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"time"
	"unicode/utf8"
)

const (
	maxEventSourceIDBytes = 128
	maxEventSubjectBytes  = 128
	maxEventPathBytes     = 512
	maxEventPaths         = 16
	maxEventSummaryBytes  = 2 * 1024
	maxRetainedEvents     = 256
	maxEventRetention     = 30 * 24 * time.Hour
	maxEventClockSkew     = 5 * time.Minute
)

var (
	ErrEventCollectionDisabled = errors.New("agent event collection is disabled")
	stableEventSourceID        = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9._:-]*$`)
	stableEventSubject         = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9._:/@+-]*$`)
	stableEventErrorClass      = regexp.MustCompile(`^[a-z][a-z0-9_.-]*$`)
	stableCommitSHA            = regexp.MustCompile(`^[0-9a-fA-F]{7,64}$`)
	bearerCredential           = regexp.MustCompile(`(?i)\bBearer[ \t]+[A-Za-z0-9._~+/=-]+`)
	assignedCredential         = regexp.MustCompile(
		`(?i)\b(token|password|passwd|secret|api[_-]?key|authorization|cookie)[ \t]*[:=][ \t]*(?:"[^"]*"|'[^']*'|[^\s,;]+)`,
	)
	highRiskCredential = regexp.MustCompile(
		`(?i)(?:\bsk-[A-Za-z0-9_-]{16,}\b|\b(?:sk|rk|pk)_(?:live|test)_[A-Za-z0-9]{12,}\b|\bglpat-[A-Za-z0-9_-]{12,}\b|\bgithub_pat_[A-Za-z0-9_]{16,}\b|\bgh[pousr]_[A-Za-z0-9]{16,}\b|\bAKIA[0-9A-Z]{16}\b|\bAIza[0-9A-Za-z_-]{20,}\b|\bxox[baprs]-[0-9A-Za-z-]{10,}\b|\b(?:secret|private)[_-]key[_-]?[A-Za-z0-9_-]{12,}\b|\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b)`,
	)
	privateKeyMarker = regexp.MustCompile(`(?i)-----BEGIN (?:[A-Z0-9 ]+ )?PRIVATE KEY-----`)
	httpURL          = regexp.MustCompile(`https?://[^\s]+`)
)

// EventPrivacyPolicy controls event collection and bounded retention. The
// product uses DefaultEventPrivacyPolicy explicitly; a zero-value policy is
// invalid rather than silently enabling collection.
type EventPrivacyPolicy struct {
	CollectionEnabled bool
	AllowSummaries    bool
	RetainLast        int
	RetainFor         time.Duration
}

// DefaultEventPrivacyPolicy returns the explicit local product policy.
func DefaultEventPrivacyPolicy() EventPrivacyPolicy {
	return EventPrivacyPolicy{
		CollectionEnabled: true,
		AllowSummaries:    false,
		RetainLast:        128,
		RetainFor:         14 * 24 * time.Hour,
	}
}

func validateEventPrivacyPolicy(policy EventPrivacyPolicy) error {
	if !policy.CollectionEnabled {
		return nil
	}
	if policy.RetainLast <= 0 || policy.RetainLast > maxRetainedEvents {
		return fmt.Errorf("agent event retention count must be between 1 and %d", maxRetainedEvents)
	}
	if policy.RetainFor <= 0 || policy.RetainFor > maxEventRetention {
		return fmt.Errorf("agent event retention age must be between 1ns and %s", maxEventRetention)
	}
	return nil
}

// NormalizeEventObservation validates the closed input contract, strips
// provider authority, normalizes project-relative paths, and redacts the one
// permitted free-text field. Rejection errors never include input content.
func NormalizeEventObservation(
	projectRoot string,
	observedAt time.Time,
	policy EventPrivacyPolicy,
	observation EventObservation,
) (EventObservation, error) {
	if err := validateEventPrivacyPolicy(policy); err != nil {
		return EventObservation{}, err
	}
	if !policy.CollectionEnabled {
		return EventObservation{}, ErrEventCollectionDisabled
	}
	if observedAt.IsZero() {
		return EventObservation{}, errors.New("agent event observation time is required")
	}
	if observation.ModelVersion != EventModelVersion {
		return EventObservation{}, errors.New("unsupported agent event model version")
	}
	if observation.SourceSequence == 0 || !observation.Kind.Valid() || !observation.Phase.Valid() {
		return EventObservation{}, errors.New("agent event identity, kind, and phase are required")
	}
	if observation.Outcome != "" && !observation.Outcome.Valid() {
		return EventObservation{}, errors.New("unsupported agent event outcome")
	}
	if observation.Notification != "" {
		if !observation.Notification.Valid() || !observation.recognizedNotification {
			return EventObservation{}, errors.New("unsupported agent event notification")
		}
		if !validNotificationEvent(observation) {
			return EventObservation{}, errors.New("agent event notification does not match lifecycle evidence")
		}
	} else if observation.recognizedNotification {
		return EventObservation{}, errors.New("agent event notification is required")
	}

	sourceID, err := normalizeEventScalar(observation.SourceID, maxEventSourceIDBytes, false)
	if err != nil || !stableEventSourceID.MatchString(sourceID) ||
		containsCredentialLike(sourceID) {
		return EventObservation{}, errors.New("invalid agent event source identity")
	}
	subject, err := normalizeEventScalar(observation.Subject, maxEventSubjectBytes, true)
	if err != nil || (subject != "" && !stableEventSubject.MatchString(subject)) ||
		containsCredentialLike(subject) || containsReasoningMarker(subject) {
		return EventObservation{}, errors.New("invalid agent event subject")
	}
	errorClass, err := normalizeEventScalar(observation.ErrorClass, 64, true)
	if err != nil || (errorClass != "" && !stableEventErrorClass.MatchString(errorClass)) ||
		containsCredentialLike(errorClass) {
		return EventObservation{}, errors.New("invalid agent event error class")
	}
	commitSHA := strings.TrimSpace(observation.CommitSHA)
	if commitSHA != "" && !stableCommitSHA.MatchString(commitSHA) {
		return EventObservation{}, errors.New("invalid agent event commit identity")
	}
	if commitSHA != "" && observation.Kind != EventCommit {
		return EventObservation{}, errors.New("agent event commit identity is not allowed for this kind")
	}
	if observation.ExitCode != nil &&
		observation.Kind != EventLifecycle && observation.Kind != EventCommand && observation.Kind != EventTest {
		return EventObservation{}, errors.New("agent event exit code is not allowed for this kind")
	}
	if errorClass != "" && observation.Kind != EventError &&
		observation.Phase != EventBlocked && observation.Phase != EventFailed {
		return EventObservation{}, errors.New("agent event error class is not allowed for this phase")
	}

	paths, err := normalizeEventPaths(projectRoot, observation.Paths)
	if err != nil {
		return EventObservation{}, err
	}
	summary := strings.TrimSpace(observation.Summary)
	if summary != "" {
		if observation.Kind != EventSummary || observation.Phase != EventCompleted ||
			!policy.AllowSummaries {
			return EventObservation{}, errors.New("agent event summaries are not allowed")
		}
		if containsReasoningMarker(summary) {
			return EventObservation{}, errors.New("agent event summary contains disallowed reasoning content")
		}
		if highRiskCredential.MatchString(summary) || privateKeyMarker.MatchString(summary) {
			return EventObservation{}, errors.New("agent event summary contains disallowed credential content")
		}
		summary = strings.Join(strings.Fields(summary), " ")
		summary = redactEventSummary(summary)
		if !validEventText(summary, true) || len(summary) > maxEventSummaryBytes {
			return EventObservation{}, errors.New("agent event summary exceeds the privacy boundary")
		}
	} else if observation.Kind == EventSummary {
		return EventObservation{}, errors.New("agent summary event requires a summary")
	}

	occurredAt := observation.OccurredAt
	if occurredAt.IsZero() {
		occurredAt = observedAt
	}
	if occurredAt.After(observedAt.Add(maxEventClockSkew)) ||
		occurredAt.Before(observedAt.Add(-maxEventRetention)) {
		return EventObservation{}, errors.New("agent event occurrence time is outside the accepted window")
	}

	normalized := observation
	normalized.SourceID = sourceID
	normalized.Subject = subject
	normalized.Paths = paths
	normalized.CommitSHA = strings.ToLower(commitSHA)
	normalized.ErrorClass = errorClass
	normalized.Summary = summary
	normalized.OccurredAt = occurredAt.UTC()
	return normalized, nil
}

func validNotificationEvent(observation EventObservation) bool {
	if observation.Subject != "" || len(observation.Paths) != 0 ||
		observation.CommitSHA != "" || observation.ExitCode != nil ||
		observation.Summary != "" {
		return false
	}
	switch observation.Notification {
	case NotificationApprovalRequested, NotificationQuestion:
		return observation.Kind == EventLifecycle && observation.Phase == EventWaiting &&
			observation.Outcome == "" && observation.ErrorClass == ""
	case NotificationFailure:
		return (observation.Kind == EventLifecycle || observation.Kind == EventError) &&
			observation.Phase == EventFailed && observation.Outcome == EventUnsuccessful
	case NotificationCompletion:
		return observation.Kind == EventLifecycle && observation.Phase == EventCompleted &&
			observation.Outcome == EventSucceeded && observation.ErrorClass == ""
	default:
		return false
	}
}

func normalizeEventScalar(value string, maximum int, optional bool) (string, error) {
	value = strings.TrimSpace(value)
	if value == "" {
		if optional {
			return "", nil
		}
		return "", errors.New("required agent event field is empty")
	}
	if len(value) > maximum || !validEventText(value, false) {
		return "", errors.New("agent event field is invalid")
	}
	return value, nil
}

func validEventText(value string, allowLines bool) bool {
	if !utf8.ValidString(value) || strings.IndexByte(value, 0) >= 0 {
		return false
	}
	for _, character := range value {
		if character < 0x20 && !(allowLines && (character == '\n' || character == '\t')) {
			return false
		}
	}
	return true
}

func normalizeEventPaths(projectRoot string, paths []string) ([]string, error) {
	if len(paths) > maxEventPaths {
		return nil, errors.New("agent event has too many paths")
	}
	root, err := filepath.Abs(projectRoot)
	if err != nil {
		return nil, errors.New("agent event project root is invalid")
	}
	root = filepath.Clean(root)
	normalized := make([]string, 0, len(paths))
	seen := make(map[string]bool, len(paths))
	for _, path := range paths {
		if len(path) == 0 || len(path) > maxEventPathBytes || !validEventText(path, false) ||
			filepath.IsAbs(path) || filepath.VolumeName(path) != "" {
			return nil, errors.New("agent event path is invalid")
		}
		clean := filepath.Clean(filepath.FromSlash(path))
		if clean == "." || clean == ".." || strings.HasPrefix(clean, ".."+string(filepath.Separator)) {
			return nil, errors.New("agent event path is outside the project")
		}
		absolute := filepath.Join(root, clean)
		relative, relErr := filepath.Rel(root, absolute)
		if relErr != nil || relative == ".." || strings.HasPrefix(relative, ".."+string(filepath.Separator)) {
			return nil, errors.New("agent event path is outside the project")
		}
		portable := filepath.ToSlash(relative)
		if containsCredentialLike(portable) || containsReasoningMarker(portable) {
			return nil, errors.New("agent event path crosses the privacy boundary")
		}
		if !seen[portable] {
			seen[portable] = true
			normalized = append(normalized, portable)
		}
	}
	sort.Strings(normalized)
	return normalized, nil
}

func containsReasoningMarker(value string) bool {
	lower := strings.ToLower(value)
	for _, marker := range []string{
		"<thinking", "</thinking", "<analysis", "</analysis",
		"chain of thought", "chain-of-thought", "internal reasoning:", "private reasoning:",
		"step-by-step reasoning", "step by step reasoning", "thought process:",
		"hidden rationale:", "private deliberation:", "scratchpad:",
		"my reasoning", "reasoning was", "i reasoned", "rationale:",
	} {
		if strings.Contains(lower, marker) {
			return true
		}
	}
	return false
}

func containsCredentialLike(value string) bool {
	return bearerCredential.MatchString(value) || assignedCredential.MatchString(value) ||
		highRiskCredential.MatchString(value) || privateKeyMarker.MatchString(value)
}

func redactEventSummary(value string) string {
	redacted := bearerCredential.ReplaceAllString(value, "Bearer [redacted]")
	redacted = assignedCredential.ReplaceAllStringFunc(redacted, func(match string) string {
		separator := strings.IndexAny(match, ":=")
		if separator < 0 {
			return "[redacted]"
		}
		return strings.TrimSpace(match[:separator]) + "=[redacted]"
	})
	redacted = httpURL.ReplaceAllStringFunc(redacted, redactEventURL)
	return redacted
}

func redactEventURL(raw string) string {
	core := strings.TrimRight(raw, ".,;:!?)\"]}")
	trailing := strings.TrimPrefix(raw, core)
	parsed, err := url.Parse(core)
	if err != nil || parsed.Scheme == "" || parsed.Host == "" {
		return "[redacted-url]" + trailing
	}
	parsed.User = nil
	if parsed.RawQuery != "" {
		parsed.RawQuery = "redacted"
	}
	parsed.Fragment = ""
	return parsed.String() + trailing
}

// RetainEvents applies count and age limits and returns independent event
// copies in canonical host-sequence order. Disabled collection returns an
// empty slice, allowing privacy controls to erase an in-memory projection.
func RetainEvents(
	events []Event,
	observedAt time.Time,
	policy EventPrivacyPolicy,
) ([]Event, error) {
	if err := validateEventPrivacyPolicy(policy); err != nil {
		return nil, err
	}
	if !policy.CollectionEnabled {
		return []Event{}, nil
	}
	cutoff := observedAt.Add(-policy.RetainFor)
	retained := make([]Event, 0, min(len(events), policy.RetainLast))
	for _, event := range events {
		if event.ObservedAt.Before(cutoff) {
			continue
		}
		clone := event
		clone.Paths = append([]string{}, event.Paths...)
		retained = append(retained, clone)
	}
	sort.SliceStable(retained, func(i, j int) bool {
		if retained[i].HostSequence != retained[j].HostSequence {
			return retained[i].HostSequence < retained[j].HostSequence
		}
		if !retained[i].ObservedAt.Equal(retained[j].ObservedAt) {
			return retained[i].ObservedAt.Before(retained[j].ObservedAt)
		}
		return retained[i].ID < retained[j].ID
	})
	if len(retained) > policy.RetainLast {
		retained = retained[len(retained)-policy.RetainLast:]
	}
	return retained, nil
}
