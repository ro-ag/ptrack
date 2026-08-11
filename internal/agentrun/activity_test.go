package agentrun

import "testing"

func TestDeriveActivityStateDecisionTable(t *testing.T) {
	tests := []struct {
		name         string
		mutateRun    func(*Run)
		intelligence IntelligenceState
		want         ActivityState
	}{
		{name: "active defaults to running", want: ActivityRunning},
		{name: "active working", intelligence: IntelligenceWorking, want: ActivityRunning},
		{name: "active drift signal remains running", intelligence: IntelligencePotentiallyDrifting, want: ActivityRunning},
		{name: "waiting comes from intelligence", intelligence: IntelligenceWaiting, want: ActivityWaiting},
		{name: "blocked comes from intelligence", intelligence: IntelligenceBlocked, want: ActivityBlocked},
		{name: "explicit completion", intelligence: IntelligenceCompleted, want: ActivityCompleted},
		{name: "explicit failure", intelligence: IntelligenceFailed, want: ActivityFailed},
		{
			name: "stale lifecycle takes precedence", mutateRun: func(run *Run) {
				run.State = StateStale
				run.LeaseState = LeaseExpired
			},
			intelligence: IntelligenceFailed, want: ActivityStale,
		},
		{
			name: "successful exit stays unknown", mutateRun: func(run *Run) {
				run.State = StateExited
				run.LeaseState = LeaseExpired
				run.Exit = &Exit{Code: 0, Result: "completed"}
			},
			want: ActivityUnknown,
		},
		{
			name: "nonzero exit is failed through intelligence", mutateRun: func(run *Run) {
				run.State = StateExited
				run.LeaseState = LeaseExpired
				run.Exit = &Exit{Code: 2, Result: "failed"}
			},
			intelligence: IntelligenceFailed, want: ActivityFailed,
		},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			run := intelligenceRun()
			if test.mutateRun != nil {
				test.mutateRun(&run)
			}
			got := DeriveActivityState(run, RunIntelligence{State: test.intelligence})
			if got != test.want {
				t.Fatalf("activity = %q, want %q", got, test.want)
			}
		})
	}
}
