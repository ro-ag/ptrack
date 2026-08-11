package agentrun

import (
	"reflect"
	"testing"
)

func TestProviderAdaptersCoverInstalledProfiles(t *testing.T) {
	if got, want := SupportedEventProviders(), []string{"agy", "claude", "codex", "gemini", "opencode"}; !reflect.DeepEqual(got, want) {
		t.Fatalf("supported providers = %#v, want %#v", got, want)
	}
	cases := []struct {
		provider string
		typeName string
		category EventKind
		kind     EventKind
		phase    EventPhase
	}{
		{provider: "codex", typeName: "turn.started", kind: EventLifecycle, phase: EventProgress},
		{provider: "codex", typeName: "PermissionRequest", kind: EventLifecycle, phase: EventWaiting},
		{provider: "codex", typeName: "item.completed", category: EventTest, kind: EventTest, phase: EventCompleted},
		{provider: "claude", typeName: "PreToolUse", kind: EventTool, phase: EventStarted},
		{provider: "claude", typeName: "PermissionRequest", kind: EventLifecycle, phase: EventWaiting},
		{provider: "gemini", typeName: "AfterTool", kind: EventTool, phase: EventCompleted},
		{provider: "gemini", typeName: "Notification", kind: EventLifecycle, phase: EventProgress},
		{provider: "agy", typeName: "session.failed", kind: EventLifecycle, phase: EventFailed},
		{provider: "opencode", typeName: "file.edited", kind: EventFile, phase: EventCompleted},
		{provider: "opencode", typeName: "session.idle", kind: EventLifecycle, phase: EventWaiting},
	}
	for _, test := range cases {
		t.Run(test.provider+"/"+test.typeName, func(t *testing.T) {
			observation, err := NormalizeProviderEvent(test.provider, ProviderEvent{
				ModelVersion: ProviderEventModelVersion,
				ID:           "provider-event-1",
				Sequence:     1,
				Type:         test.typeName,
				Category:     test.category,
				Subject:      "safe-label",
			})
			if err != nil {
				t.Fatal(err)
			}
			if observation.Kind != test.kind || observation.Phase != test.phase ||
				observation.ModelVersion != EventModelVersion ||
				observation.SourceID != "provider-event-1" {
				t.Fatalf("normalized observation = %#v", observation)
			}
		})
	}
}

func TestProviderAdaptersReserveCompletionForExplicitSessionEnd(t *testing.T) {
	cases := []struct {
		provider string
		typeName string
		phase    EventPhase
	}{
		{provider: "codex", typeName: "turn.completed", phase: EventWaiting},
		{provider: "codex", typeName: "stop", phase: EventWaiting},
		{provider: "codex", typeName: "sessionend", phase: EventCompleted},
		{provider: "claude", typeName: "stop", phase: EventWaiting},
		{provider: "claude", typeName: "sessionend", phase: EventCompleted},
	}
	for _, test := range cases {
		t.Run(test.provider+"/"+test.typeName, func(t *testing.T) {
			observation, err := NormalizeProviderEvent(test.provider, ProviderEvent{
				ModelVersion: ProviderEventModelVersion,
				ID:           "event-1",
				Sequence:     1,
				Type:         test.typeName,
			})
			if err != nil {
				t.Fatal(err)
			}
			if observation.Phase != test.phase {
				t.Fatalf("phase = %q, want %q", observation.Phase, test.phase)
			}
		})
	}

	opencode, err := NormalizeProviderEvent("opencode", ProviderEvent{
		ModelVersion: ProviderEventModelVersion,
		ID:           "error-1",
		Sequence:     1,
		Type:         "session.error",
	})
	if err != nil {
		t.Fatal(err)
	}
	if opencode.ErrorClass != "session_failure" {
		t.Fatalf("OpenCode session error class = %q", opencode.ErrorClass)
	}
}

func TestKnownProvidersAcceptCanonicalWrapperEvents(t *testing.T) {
	exitCode := 1
	observation, err := NormalizeProviderEvent("gemini", ProviderEvent{
		ModelVersion: ProviderEventModelVersion,
		ID:           "command-1",
		Sequence:     4,
		Type:         "command.completed",
		Subject:      "go",
		ExitCode:     &exitCode,
	})
	if err != nil {
		t.Fatal(err)
	}
	if observation.Kind != EventCommand || observation.Phase != EventFailed ||
		observation.Outcome != EventUnsuccessful {
		t.Fatalf("exit-aware canonical observation = %#v", observation)
	}
}

func TestProviderAdaptersRejectSelfAssertedSummaries(t *testing.T) {
	for _, provider := range SupportedEventProviders() {
		_, err := NormalizeProviderEvent(provider, ProviderEvent{
			ModelVersion: ProviderEventModelVersion,
			ID:           "summary-1", Sequence: 1, Type: "summary.completed",
			Summary: "agent-provided text",
		})
		if err == nil {
			t.Fatalf("provider %q self-asserted a final summary", provider)
		}
	}
}

func TestFutureProviderFallbackIsLifecycleOnly(t *testing.T) {
	base := ProviderEvent{
		ModelVersion: ProviderEventModelVersion,
		ID:           "future-1",
		Sequence:     1,
		Type:         "lifecycle.progress",
	}
	if _, err := NormalizeProviderEvent("future-agent", base); err != nil {
		t.Fatalf("future lifecycle fallback: %v", err)
	}
	base.Type = "tool.started"
	if _, err := NormalizeProviderEvent("future-agent", base); err == nil {
		t.Fatal("future provider accepted non-lifecycle evidence without an adapter")
	}
}

func TestProviderAdaptersFailClosed(t *testing.T) {
	base := ProviderEvent{
		ModelVersion: ProviderEventModelVersion,
		ID:           "event-1",
		Sequence:     1,
		Type:         "turn.started",
	}
	invalid := []struct {
		provider string
		event    ProviderEvent
	}{
		{provider: "", event: base},
		{provider: "bad provider", event: base},
		{provider: "codex", event: func() ProviderEvent { value := base; value.ModelVersion++; return value }()},
		{provider: "codex", event: func() ProviderEvent { value := base; value.Sequence = 0; return value }()},
		{provider: "codex", event: func() ProviderEvent { value := base; value.Type = "prompt"; return value }()},
		{provider: "codex", event: func() ProviderEvent { value := base; value.Category = EventTool; return value }()},
	}
	for _, test := range invalid {
		if _, err := NormalizeProviderEvent(test.provider, test.event); err == nil {
			t.Fatalf("accepted invalid provider event %#v", test)
		}
	}
}
