package gui

import (
	"context"
	"encoding/json"
	"strings"
	"testing"

	"github.com/ro-ag/ptrack/internal/agentrun"
	"github.com/ro-ag/ptrack/internal/association"
)

func TestGetAgentRunsV2ReturnsOnlyHostValidatedContentFreeSummaries(t *testing.T) {
	app, projectRoot := newTerminalBindingTestApp(t, &fakeGUITerminalManager{}, nil)
	planID, taskID, _ := seedAssociationCatalog(t, projectRoot)
	registry := agentrun.NewRegistry(agentrun.Config{ProjectRoot: projectRoot})
	t.Cleanup(func() { _ = registry.Shutdown(context.Background()) })
	app.workspace.agents = registry
	lease, err := registry.RegisterExternal(agentrun.Registration{
		Profile:  "RAW_PROFILE_SECRET_CANARY",
		Provider: "RAW_PROVIDER_SECRET_CANARY",
		PID:      777,
		CWD:      projectRoot + "/RAW_CWD_SECRET_CANARY",
	})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := app.AssociateAgentRunV2(1, lease.Run.ID, association.PointerV1{
		Version: association.VersionV1, PlanID: planID, TaskID: taskID,
	}); err != nil {
		t.Fatal(err)
	}
	if err := registry.ExitExternal(
		lease.Run.ID, lease.LeaseToken, 7, "RAW_RESULT_SECRET_CANARY",
	); err != nil {
		t.Fatal(err)
	}

	result, err := app.GetAgentRunsV2(1)
	if err != nil {
		t.Fatal(err)
	}
	if result.Generation != 1 || result.Bounds.Shown != 1 || result.Bounds.Total != 1 ||
		len(result.Runs) != 1 || result.Runs[0].RunID != lease.Run.ID ||
		result.Runs[0].Association == nil ||
		result.Runs[0].Association.PlanID != planID ||
		result.Runs[0].Association.TaskID != taskID || result.Runs[0].Live {
		t.Fatalf("content-free AgentRun result = %#v", result)
	}
	encoded, err := json.Marshal(result)
	if err != nil {
		t.Fatal(err)
	}
	for _, forbidden := range []string{
		"RAW_PROFILE_SECRET_CANARY", "RAW_PROVIDER_SECRET_CANARY",
		"RAW_CWD_SECRET_CANARY", "RAW_RESULT_SECRET_CANARY",
		`"profile"`, `"provider"`, `"pid"`, `"cwd"`, `"projectRoot"`,
		`"exit"`, `"result"`, `"liveId"`,
	} {
		if strings.Contains(string(encoded), forbidden) {
			t.Fatalf("AgentRun DTO contains forbidden %q: %s", forbidden, encoded)
		}
	}
}
