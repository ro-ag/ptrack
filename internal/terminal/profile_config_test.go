package terminal

import (
	"encoding/json"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
)

func TestValidateProfileNormalizesSafePresentationAndPolicyDefaults(t *testing.T) {
	input := configuredTestProfile(t, "shell-default", ProfileShell)
	input.Args = []string{"-l"}
	input.Env = map[string]string{"PAGER": "less"}

	got, err := ValidateProfile(input)
	if err != nil {
		t.Fatalf("ValidateProfile: %v", err)
	}
	if got.Theme != DefaultProfileTheme || got.FontFamily != DefaultProfileFontFamily ||
		got.FontSize != DefaultProfileFontSize || got.Scrollback != DefaultProfileScrollback ||
		got.CWDPolicy != CWDRequested || got.FixedCWD != "" || got.ExitBehavior != ExitKeep {
		t.Fatalf("normalized profile defaults = %#v", got)
	}
	if input.Theme != "" || input.FontFamily != "" || input.FontSize != 0 ||
		input.Scrollback != 0 || input.CWDPolicy != "" || input.ExitBehavior != "" {
		t.Fatalf("ValidateProfile mutated caller defaults: %#v", input)
	}

	input.Args[0] = "changed"
	input.Env["PAGER"] = "changed"
	if got.Args[0] != "-l" || got.Env["PAGER"] != "less" {
		t.Fatalf("validated profile aliases caller data: %#v", got)
	}
}

func TestValidateProfileBoundsSettingsAndRejectsAuthorityOrSecretEnvironment(t *testing.T) {
	fixed := t.TempDir()
	valid := configuredTestProfile(t, "profile", ProfileShell)
	valid.Theme = "solarized-dark"
	valid.FontFamily = "Iosevka, monospace"
	valid.FontSize = MinProfileFontSize
	valid.Scrollback = MaxProfileScrollback
	valid.CWDPolicy = CWDFixed
	valid.FixedCWD = fixed
	valid.ExitBehavior = ExitCloseOnSuccess
	if _, err := ValidateProfile(valid); err != nil {
		t.Fatalf("ValidateProfile valid settings: %v", err)
	}

	tests := []struct {
		name   string
		change func(*Profile)
	}{
		{name: "theme", change: func(profile *Profile) { profile.Theme = "not a stable theme" }},
		{name: "font size", change: func(profile *Profile) { profile.FontSize = MaxProfileFontSize + 1 }},
		{name: "scrollback", change: func(profile *Profile) { profile.Scrollback = MaxProfileScrollback + 1 }},
		{name: "relative fixed CWD", change: func(profile *Profile) {
			profile.CWDPolicy, profile.FixedCWD = CWDFixed, "relative"
		}},
		{name: "unused fixed CWD", change: func(profile *Profile) {
			profile.CWDPolicy, profile.FixedCWD = CWDProject, fixed
		}},
		{name: "exit behavior", change: func(profile *Profile) { profile.ExitBehavior = "restart" }},
		{name: "argument count", change: func(profile *Profile) {
			profile.Args = make([]string, maxProfileArgumentCount+1)
		}},
		{name: "reserved environment", change: func(profile *Profile) {
			profile.Env = map[string]string{"PTRACK_CAPABILITY_TOKEN": "value"}
		}},
		{name: "case-insensitive reserved environment", change: func(profile *Profile) {
			profile.Env = map[string]string{"ptrack_custom": "value"}
		}},
		{name: "secret environment", change: func(profile *Profile) {
			profile.Env = map[string]string{"OPENAI_API_KEY": "value"}
		}},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			profile := configuredTestProfile(t, "profile", ProfileShell)
			test.change(&profile)
			if _, err := ValidateProfile(profile); err == nil {
				t.Fatal("ValidateProfile accepted invalid bounded configuration")
			}
		})
	}
}

func TestMergeProfilesReplacesStableIDsAddsCustomProfilesAndOwnsCopies(t *testing.T) {
	discoveredShell := configuredTestProfile(t, "shell-default", ProfileShell)
	discoveredShell.Name = "Default shell"
	discoveredShell.Args = []string{"-l"}
	discoveredShell.Env = map[string]string{"PAGER": "less"}
	discoveredAgent := configuredTestProfile(t, "agent-codex", ProfileAgent)
	discoveredAgent.Name = "Codex"
	discoveredAgent.Provider = "codex"
	discoveredAgent.Args = []string{"--discovered"}
	discoveredAgent.Env = map[string]string{"CODEX_MODE": "base"}

	override := discoveredAgent
	override.Name = "Codex focused"
	override.Theme = "solarized-dark"
	override.FontSize = MaxProfileFontSize
	override.ExitBehavior = ExitCloseOnSuccess
	custom := configuredTestProfile(t, "shell-tools", ProfileShell)
	custom.Name = "Tools shell"
	custom.Args = []string{"--custom"}
	custom.Env = map[string]string{"PAGER": "more"}

	merged, err := MergeProfiles(
		[]Profile{discoveredAgent, discoveredShell},
		[]Profile{custom, override},
	)
	if err != nil {
		t.Fatalf("MergeProfiles: %v", err)
	}
	wantIDs := []string{"shell-default", "shell-tools", "agent-codex"}
	gotIDs := make([]string, 0, len(merged))
	for _, profile := range merged {
		gotIDs = append(gotIDs, profile.ID)
	}
	if !reflect.DeepEqual(gotIDs, wantIDs) {
		t.Fatalf("merged IDs = %v, want %v", gotIDs, wantIDs)
	}
	gotAgent := merged[2]
	if gotAgent.Name != override.Name || !reflect.DeepEqual(gotAgent.Args, discoveredAgent.Args) ||
		gotAgent.Env["CODEX_MODE"] != "base" || gotAgent.Theme != "solarized-dark" ||
		gotAgent.FontSize != MaxProfileFontSize || gotAgent.ExitBehavior != ExitCloseOnSuccess {
		t.Fatalf("configured override = %#v", gotAgent)
	}
	if !reflect.DeepEqual(merged[1].Args, custom.Args) || merged[1].Env["PAGER"] != "more" {
		t.Fatalf("custom shell launch settings = %#v", merged[1])
	}

	override.Args[0] = "changed"
	override.Env["CODEX_MODE"] = "changed"
	custom.Args[0] = "changed"
	custom.Env["PAGER"] = "changed"
	discoveredShell.Args[0] = "changed"
	discoveredShell.Env["PAGER"] = "changed"
	if gotAgent.Args[0] != "--discovered" || gotAgent.Env["CODEX_MODE"] != "base" ||
		merged[0].Args[0] != "-l" || merged[0].Env["PAGER"] != "less" ||
		merged[1].Args[0] != "--custom" || merged[1].Env["PAGER"] != "more" {
		t.Fatalf("merged profiles alias input data: %#v", merged)
	}
}

func TestMergeProfilesRejectsDuplicateAndRepurposedIdentities(t *testing.T) {
	discovered := configuredTestProfile(t, "agent-codex", ProfileAgent)
	discovered.Provider = "codex"

	duplicate := configuredTestProfile(t, "custom", ProfileShell)
	if _, err := MergeProfiles(nil, []Profile{duplicate, duplicate}); err == nil {
		t.Fatal("MergeProfiles accepted duplicate configured IDs")
	}

	changedKind := discovered
	changedKind.Kind = ProfileShell
	changedKind.Provider = ""
	if _, err := MergeProfiles([]Profile{discovered}, []Profile{changedKind}); err == nil {
		t.Fatal("MergeProfiles accepted a changed discovered kind")
	}

	changedProvider := discovered
	changedProvider.Provider = "other"
	if _, err := MergeProfiles([]Profile{discovered}, []Profile{changedProvider}); err == nil {
		t.Fatal("MergeProfiles accepted a changed discovered provider")
	}

	changedExecutable := discovered
	changedExecutable.Executable = filepath.Join(t.TempDir(), "other")
	if _, err := MergeProfiles([]Profile{discovered}, []Profile{changedExecutable}); err == nil {
		t.Fatal("MergeProfiles accepted a changed discovered agent executable")
	}

	changedArgs := discovered
	changedArgs.Args = []string{"--changed"}
	if _, err := MergeProfiles([]Profile{discovered}, []Profile{changedArgs}); err == nil {
		t.Fatal("MergeProfiles accepted changed discovered agent arguments")
	}

	changedEnvironment := discovered
	changedEnvironment.Env = map[string]string{"DYLD_INSERT_LIBRARIES": "/tmp/injected.dylib"}
	if _, err := MergeProfiles([]Profile{discovered}, []Profile{changedEnvironment}); err == nil {
		t.Fatal("MergeProfiles accepted changed discovered agent environment")
	}

	changedCWD := discovered
	changedCWD.CWDPolicy = CWDProject
	if _, err := MergeProfiles([]Profile{discovered}, []Profile{changedCWD}); err == nil {
		t.Fatal("MergeProfiles accepted changed discovered agent working-directory policy")
	}

	fixedCWD := discovered
	fixedCWD.CWDPolicy = CWDFixed
	fixedCWD.FixedCWD = t.TempDir()
	changedFixedCWD := fixedCWD
	changedFixedCWD.FixedCWD = t.TempDir()
	if _, err := MergeProfiles([]Profile{fixedCWD}, []Profile{changedFixedCWD}); err == nil {
		t.Fatal("MergeProfiles accepted changed discovered agent fixed working directory")
	}

	customAgent := configuredTestProfile(t, "agent-custom", ProfileAgent)
	if _, err := MergeProfiles(nil, []Profile{customAgent}); err == nil {
		t.Fatal("MergeProfiles accepted a custom configured agent identity")
	}
}

func TestProfileConfigPrivateAtomicRoundTripDoesNotSnapshotInheritedEnvironment(t *testing.T) {
	directory := t.TempDir()
	path := filepath.Join(directory, "profiles.json")
	t.Setenv("PROFILE_CONFIG_PARENT_SECRET", "must-not-persist")

	first := configuredTestProfile(t, "shell-default", ProfileShell)
	first.Name = "First"
	first.Env = map[string]string{"PAGER": "less"}
	config := ProfileConfig{Version: ProfileConfigVersion, Profiles: []Profile{first}}
	if err := SaveProfileConfig(path, config); err != nil {
		t.Fatalf("SaveProfileConfig first: %v", err)
	}

	second := first
	second.Name = "Second"
	second.Env = map[string]string{"PAGER": "more"}
	if err := SaveProfileConfig(path, ProfileConfig{
		Version: ProfileConfigVersion, Profiles: []Profile{second},
	}); err != nil {
		t.Fatalf("SaveProfileConfig replacement: %v", err)
	}

	assertProfileConfigPrivate(t, path)
	contents, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read profile config: %v", err)
	}
	if strings.Contains(string(contents), "PROFILE_CONFIG_PARENT_SECRET") ||
		strings.Contains(string(contents), "must-not-persist") {
		t.Fatal("profile config snapshots inherited environment")
	}
	entries, err := os.ReadDir(directory)
	if err != nil {
		t.Fatalf("read config directory: %v", err)
	}
	if len(entries) != 1 || entries[0].Name() != filepath.Base(path) {
		t.Fatalf("atomic save left temporary files: %v", entries)
	}

	loaded, err := LoadProfileConfig(path)
	if err != nil {
		t.Fatalf("LoadProfileConfig: %v", err)
	}
	if loaded.Version != ProfileConfigVersion || len(loaded.Profiles) != 1 ||
		loaded.Profiles[0].Name != "Second" || loaded.Profiles[0].Env["PAGER"] != "more" ||
		loaded.Profiles[0].Theme != DefaultProfileTheme {
		t.Fatalf("loaded profile config = %#v", loaded)
	}

	second.Env["PAGER"] = "changed"
	if loaded.Profiles[0].Env["PAGER"] != "more" {
		t.Fatal("loaded profile config aliases save input")
	}
}

func TestLoadProfileConfigRejectsUnknownVersionFieldsTrailingAndOversize(t *testing.T) {
	directory := t.TempDir()
	profile := configuredTestProfile(t, "shell", ProfileShell)
	profileJSON, err := json.Marshal(profile)
	if err != nil {
		t.Fatal(err)
	}
	tests := []struct {
		name string
		body string
	}{
		{name: "version", body: `{"version":2,"profiles":[]}`},
		{name: "unknown", body: `{"version":1,"profiles":[],"extra":true}`},
		{name: "trailing", body: `{"version":1,"profiles":[]} {}`},
		{name: "invalid profile", body: `{"version":1,"profiles":[` + string(profileJSON[:len(profileJSON)-1]) + `]}`},
		{name: "oversize", body: strings.Repeat(" ", maxProfileConfigJSONBytes+1)},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			path := filepath.Join(directory, test.name+".json")
			if err := os.WriteFile(path, []byte(test.body), 0o600); err != nil {
				t.Fatal(err)
			}
			if _, err := LoadProfileConfig(path); err == nil {
				t.Fatal("LoadProfileConfig accepted invalid input")
			}
		})
	}
}

func TestManagerEnforcesProfileWorkingDirectoryPolicy(t *testing.T) {
	projectRoot := t.TempDir()
	requested := t.TempDir()
	fixed := t.TempDir()
	projectProcess := newManagerFakeProcess()
	fixedProcess := newManagerFakeProcess()
	factory := newManagerFakeFactory(
		managerStartOutcome{process: projectProcess},
		managerStartOutcome{process: fixedProcess},
	)
	projectProfile := configuredTestProfile(t, "shell-project", ProfileShell)
	projectProfile.CWDPolicy = CWDProject
	fixedProfile := configuredTestProfile(t, "shell-fixed", ProfileShell)
	fixedProfile.CWDPolicy = CWDFixed
	fixedProfile.FixedCWD = fixed
	manager, err := NewManager(projectRoot, []Profile{projectProfile, fixedProfile}, factory)
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}
	cleanupManager(t, manager, projectProcess, fixedProcess)

	if _, err := manager.Create(projectProfile.ID, requested, 24, 80); err != nil {
		t.Fatalf("create project-policy terminal: %v", err)
	}
	if _, err := manager.Create(fixedProfile.ID, requested, 24, 80); err != nil {
		t.Fatalf("create fixed-policy terminal: %v", err)
	}
	starts := factory.recordedStarts()
	if len(starts) != 2 || starts[0].CWD != projectRoot || starts[1].CWD != fixed {
		t.Fatalf("profile working directories = %#v", starts)
	}
}

func configuredTestProfile(t *testing.T, id string, kind ProfileKind) Profile {
	t.Helper()
	profile := Profile{
		ID:         id,
		Name:       id,
		Kind:       kind,
		Executable: filepath.Join(t.TempDir(), id),
	}
	if kind == ProfileAgent {
		profile.Provider = strings.TrimPrefix(id, "agent-")
		if profile.Provider == "" {
			profile.Provider = "test"
		}
	}
	return profile
}
