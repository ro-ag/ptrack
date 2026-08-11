package agentrun

// ActivityState is the small, content-free state vocabulary used by unified
// agent activity views. Unknown is retained explicitly so a successful
// process exit cannot be mistaken for objective completion.
type ActivityState string

const (
	ActivityRunning   ActivityState = "running"
	ActivityWaiting   ActivityState = "waiting"
	ActivityBlocked   ActivityState = "blocked"
	ActivityCompleted ActivityState = "completed"
	ActivityFailed    ActivityState = "failed"
	ActivityStale     ActivityState = "stale"
	ActivityUnknown   ActivityState = "unknown"
)

// DeriveActivityState combines lifecycle and structured intelligence without
// introducing a second inference model. Lifecycle staleness is authoritative;
// waiting and blocked require intelligence; active runs otherwise stay
// running. Completion and failure require the explicit evidence or process
// failure semantics already enforced by DeriveRunIntelligence.
func DeriveActivityState(run Run, intelligence RunIntelligence) ActivityState {
	if run.State == StateStale {
		return ActivityStale
	}
	active := runIsActive(run)
	switch intelligence.State {
	case IntelligenceFailed:
		return ActivityFailed
	case IntelligenceCompleted:
		return ActivityCompleted
	case IntelligenceBlocked:
		if active {
			return ActivityBlocked
		}
	case IntelligenceWaiting:
		if active {
			return ActivityWaiting
		}
	}
	if active {
		return ActivityRunning
	}
	return ActivityUnknown
}
