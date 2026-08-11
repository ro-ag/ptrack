package agentrun

import (
	"errors"
	"regexp"
	"sort"
	"strings"
	"time"
)

const ProviderEventModelVersion uint = 1

var stableProviderEventName = regexp.MustCompile(`^[A-Za-z][A-Za-z0-9._-]*$`)

// ProviderEvent is the closed hook/wrapper input shared by provider adapters.
// Provider identity is intentionally absent: the authenticated Run supplies
// it. Adapters never accept native raw JSON, prompts, messages, tool payloads,
// command arguments, output, transcripts, environment, or credentials.
type ProviderEvent struct {
	ModelVersion uint      `json:"modelVersion"`
	ID           string    `json:"id"`
	Sequence     uint64    `json:"sequence"`
	Type         string    `json:"type"`
	Category     EventKind `json:"category,omitempty"`
	Subject      string    `json:"subject,omitempty"`
	Paths        []string  `json:"paths,omitempty"`
	CommitSHA    string    `json:"commitSha,omitempty"`
	ExitCode     *int      `json:"exitCode,omitempty"`
	ErrorClass   string    `json:"errorClass,omitempty"`
	Summary      string    `json:"summary,omitempty"`
	OccurredAt   time.Time `json:"occurredAt,omitempty"`
}

type adapterMapping struct {
	kind         EventKind
	phase        EventPhase
	outcome      EventOutcome
	errorClass   string
	notification EventNotificationKind
}

var providerEventAliases = map[string]map[string]adapterMapping{
	"codex": {
		"sessionstart":       {kind: EventLifecycle, phase: EventStarted},
		"turn.started":       {kind: EventLifecycle, phase: EventProgress},
		"turn.completed":     {kind: EventLifecycle, phase: EventWaiting},
		"turn.failed":        {kind: EventError, phase: EventFailed, outcome: EventUnsuccessful, notification: NotificationFailure},
		"pretooluse":         {kind: EventTool, phase: EventStarted},
		"posttooluse":        {kind: EventTool, phase: EventCompleted, outcome: EventSucceeded},
		"posttoolusefailure": {kind: EventTool, phase: EventFailed, outcome: EventUnsuccessful},
		"permissionrequest":  {kind: EventLifecycle, phase: EventWaiting, notification: NotificationApprovalRequested},
		"question":           {kind: EventLifecycle, phase: EventWaiting, notification: NotificationQuestion},
		"stop":               {kind: EventLifecycle, phase: EventWaiting},
		"sessionend":         {kind: EventLifecycle, phase: EventCompleted, outcome: EventSucceeded, notification: NotificationCompletion},
	},
	"claude": {
		"sessionstart":       {kind: EventLifecycle, phase: EventStarted},
		"pretooluse":         {kind: EventTool, phase: EventStarted},
		"posttooluse":        {kind: EventTool, phase: EventCompleted, outcome: EventSucceeded},
		"posttoolusefailure": {kind: EventTool, phase: EventFailed, outcome: EventUnsuccessful},
		"permissionrequest":  {kind: EventLifecycle, phase: EventWaiting, notification: NotificationApprovalRequested},
		"question":           {kind: EventLifecycle, phase: EventWaiting, notification: NotificationQuestion},
		"stop":               {kind: EventLifecycle, phase: EventWaiting},
		"sessionend":         {kind: EventLifecycle, phase: EventCompleted, outcome: EventSucceeded, notification: NotificationCompletion},
	},
	"gemini": {
		"sessionstart":      {kind: EventLifecycle, phase: EventStarted},
		"beforetool":        {kind: EventTool, phase: EventStarted},
		"aftertool":         {kind: EventTool, phase: EventCompleted, outcome: EventSucceeded},
		"notification":      {kind: EventLifecycle, phase: EventProgress},
		"permissionrequest": {kind: EventLifecycle, phase: EventWaiting, notification: NotificationApprovalRequested},
		"question":          {kind: EventLifecycle, phase: EventWaiting, notification: NotificationQuestion},
		"sessionend":        {kind: EventLifecycle, phase: EventCompleted, outcome: EventSucceeded, notification: NotificationCompletion},
	},
	"agy": {
		"session.started":      {kind: EventLifecycle, phase: EventStarted},
		"session.waiting":      {kind: EventLifecycle, phase: EventWaiting},
		"session.completed":    {kind: EventLifecycle, phase: EventCompleted, outcome: EventSucceeded, notification: NotificationCompletion},
		"session.failed":       {kind: EventLifecycle, phase: EventFailed, outcome: EventUnsuccessful, notification: NotificationFailure},
		"session.question":     {kind: EventLifecycle, phase: EventWaiting, notification: NotificationQuestion},
		"permission.requested": {kind: EventLifecycle, phase: EventWaiting, notification: NotificationApprovalRequested},
	},
	"opencode": {
		"session.created":      {kind: EventLifecycle, phase: EventStarted},
		"session.idle":         {kind: EventLifecycle, phase: EventWaiting},
		"session.error":        {kind: EventError, phase: EventFailed, outcome: EventUnsuccessful, errorClass: "session_failure", notification: NotificationFailure},
		"session.completed":    {kind: EventLifecycle, phase: EventCompleted, outcome: EventSucceeded, notification: NotificationCompletion},
		"session.question":     {kind: EventLifecycle, phase: EventWaiting, notification: NotificationQuestion},
		"permission.requested": {kind: EventLifecycle, phase: EventWaiting, notification: NotificationApprovalRequested},
		"tool.execute.before":  {kind: EventTool, phase: EventStarted},
		"tool.execute.after":   {kind: EventTool, phase: EventCompleted, outcome: EventSucceeded},
		"file.edited":          {kind: EventFile, phase: EventCompleted, outcome: EventSucceeded},
		"command.executed":     {kind: EventCommand, phase: EventCompleted, outcome: EventSucceeded},
	},
}

// SupportedEventProviders returns the installed provider-adapter identities
// in deterministic order. Unknown future providers receive only the generic,
// explicit lifecycle fallback.
func SupportedEventProviders() []string {
	providers := make([]string, 0, len(providerEventAliases))
	for provider := range providerEventAliases {
		providers = append(providers, provider)
	}
	sort.Strings(providers)
	return providers
}

// NormalizeProviderEvent maps one allowlisted provider hook event into the
// provider-neutral observation contract. Known providers may also send the
// canonical `<kind>.<phase>` form through an explicit wrapper. Unknown future
// providers are lifecycle-only until an adapter is deliberately added.
func NormalizeProviderEvent(provider string, input ProviderEvent) (EventObservation, error) {
	provider = strings.ToLower(strings.TrimSpace(provider))
	if provider == "" || len(provider) > 64 || !stableProviderEventName.MatchString(provider) {
		return EventObservation{}, errors.New("invalid agent event provider")
	}
	if input.ModelVersion != ProviderEventModelVersion || input.Sequence == 0 {
		return EventObservation{}, errors.New("unsupported provider event contract")
	}
	eventType := strings.TrimSpace(input.Type)
	if eventType == "" || len(eventType) > 64 || !stableProviderEventName.MatchString(eventType) {
		return EventObservation{}, errors.New("invalid provider event type")
	}

	mapping, mapped := providerEventAliases[provider][strings.ToLower(eventType)]
	if provider == "codex" && !mapped {
		mapping, mapped = normalizeCodexItem(eventType, input.Category)
	}
	if !mapped {
		mapping, mapped = canonicalProviderEvent(eventType)
		if mapped {
			if mapping.kind == EventSummary {
				return EventObservation{}, errors.New("provider summaries require an explicit trusted adapter")
			}
			_, knownProvider := providerEventAliases[provider]
			if !knownProvider && mapping.kind != EventLifecycle {
				return EventObservation{}, errors.New("future provider adapter is lifecycle-only")
			}
		}
	}
	if input.Category != "" && !(provider == "codex" && strings.HasPrefix(strings.ToLower(eventType), "item.")) {
		return EventObservation{}, errors.New("provider event category is not allowed for this type")
	}
	if input.ExitCode != nil && *input.ExitCode != 0 &&
		(mapping.kind == EventLifecycle || mapping.kind == EventCommand || mapping.kind == EventTest) {
		mapping.phase = EventFailed
		mapping.outcome = EventUnsuccessful
		if mapping.notification != "" {
			mapping.notification = NotificationFailure
		}
	}
	if mapping.notification != "" {
		return EventObservation{
			ModelVersion: EventModelVersion, SourceID: input.ID,
			SourceSequence: input.Sequence, Kind: mapping.kind, Phase: mapping.phase,
			Outcome: mapping.outcome, ErrorClass: mapping.errorClass,
			OccurredAt: input.OccurredAt, Notification: mapping.notification,
			recognizedNotification: true,
		}, nil
	}
	if !mapped {
		return EventObservation{}, errors.New("unsupported provider event type")
	}
	errorClass := input.ErrorClass
	if errorClass == "" {
		errorClass = mapping.errorClass
	}
	return EventObservation{
		ModelVersion:   EventModelVersion,
		SourceID:       input.ID,
		SourceSequence: input.Sequence,
		Kind:           mapping.kind,
		Phase:          mapping.phase,
		Outcome:        mapping.outcome,
		Subject:        input.Subject,
		Paths:          append([]string{}, input.Paths...),
		CommitSHA:      input.CommitSHA,
		ExitCode:       cloneInt(input.ExitCode),
		ErrorClass:     errorClass,
		Summary:        input.Summary,
		OccurredAt:     input.OccurredAt,
	}, nil
}

func normalizeCodexItem(eventType string, category EventKind) (adapterMapping, bool) {
	var phase EventPhase
	var outcome EventOutcome
	switch strings.ToLower(eventType) {
	case "item.started":
		phase = EventStarted
	case "item.completed":
		phase = EventCompleted
		outcome = EventSucceeded
	case "item.failed":
		phase = EventFailed
		outcome = EventUnsuccessful
	default:
		return adapterMapping{}, false
	}
	if !category.Valid() || category == EventLifecycle || category == EventSummary {
		return adapterMapping{}, false
	}
	return adapterMapping{kind: category, phase: phase, outcome: outcome}, true
}

func canonicalProviderEvent(eventType string) (adapterMapping, bool) {
	kindName, phaseName, found := strings.Cut(strings.ToLower(eventType), ".")
	if !found || strings.Contains(phaseName, ".") {
		return adapterMapping{}, false
	}
	kind := EventKind(kindName)
	phase := EventPhase(phaseName)
	if !kind.Valid() || !phase.Valid() {
		return adapterMapping{}, false
	}
	outcome := EventOutcome("")
	if phase == EventCompleted {
		outcome = EventSucceeded
	} else if phase == EventFailed {
		outcome = EventUnsuccessful
	}
	notification := EventNotificationKind("")
	if kind == EventLifecycle && phase == EventCompleted {
		notification = NotificationCompletion
	} else if kind == EventLifecycle && phase == EventFailed {
		notification = NotificationFailure
	}
	return adapterMapping{
		kind: kind, phase: phase, outcome: outcome, notification: notification,
	}, true
}
