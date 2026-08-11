package gui

import (
	"encoding/json"
	"strings"
	"testing"

	"github.com/ro-ag/ptrack/internal/agentrun"
	"github.com/ro-ag/ptrack/internal/terminal"
)

func TestBuildAgentActivitySnapshotCountsOneRowPerRun(t *testing.T) {
	projection := runtimeProjection{
		terminals: []TerminalRuntimeSummary{
			{SessionID: "terminal-1", ProfileKind: terminal.ProfileAgent, Live: true},
		},
		agents: []AgentRuntimeSummary{
			{
				RunID: "run-1", RegistrationKind: agentrun.RegistrationLaunched,
				TerminalBacked: true, TerminalPresent: true, CorrespondingTerminal: true,
				Live: true, ActivityState: agentrun.ActivityWaiting,
				Association: &RuntimeAssociation{PlanID: 5, TaskID: 37, Revision: 2},
				Intelligence: &AgentIntelligenceSummary{
					State: agentrun.IntelligenceWaiting, Confidence: agentrun.ConfidenceMedium,
					EvidenceCount: 1, EventCount: 3, LastEventAt: "2026-08-10T20:00:00Z",
				},
			},
			{
				RunID: "run-2", RegistrationKind: agentrun.RegistrationExternal,
				ActivityState: agentrun.ActivityUnknown,
			},
		},
		agentBounds: BoundedSnapshot{Shown: 2, Total: 4, More: 2},
	}

	activity := buildAgentActivitySnapshot(projection)
	if len(activity.Items) != 2 || activity.Bounds.More != 2 {
		t.Fatalf("activity bounds = %#v items=%#v", activity.Bounds, activity.Items)
	}
	if activity.Counts.Waiting != 1 || activity.Counts.Unknown != 1 {
		t.Fatalf("activity counts = %#v", activity.Counts)
	}
	if activity.Items[0].State != agentrun.ActivityWaiting ||
		!activity.Items[0].TerminalBacked || activity.Items[0].EventCount != 3 {
		t.Fatalf("terminal-backed activity = %#v", activity.Items[0])
	}
	if len(activity.Items) != len(projection.agents) {
		t.Fatal("the corresponding terminal was counted as another agent")
	}
}

func TestAgentActivitySnapshotContainsNoRawRuntimeContent(t *testing.T) {
	activity := buildAgentActivitySnapshot(runtimeProjection{
		agents: []AgentRuntimeSummary{{
			RunID: "run-1", ActivityState: agentrun.ActivityRunning,
			Intelligence: &AgentIntelligenceSummary{
				State: agentrun.IntelligenceWorking, Confidence: agentrun.ConfidenceLow,
				EvidenceCount: 1, EventCount: 2,
			},
		}},
		agentBounds: BoundedSnapshot{Shown: 1, Total: 1},
	})
	encoded, err := json.Marshal(activity)
	if err != nil {
		t.Fatal(err)
	}
	for _, forbidden := range []string{
		`"terminalId"`, `"projectRoot"`, `"cwd"`, `"exit"`, `"result"`,
		`"events"`, `"evidence"`, `"summary"`, `"paths"`, `"token"`,
	} {
		if strings.Contains(string(encoded), forbidden) {
			t.Fatalf("activity DTO contains forbidden %q: %s", forbidden, encoded)
		}
	}
}
