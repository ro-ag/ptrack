package terminal

import (
	"errors"
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"strings"
	"testing"
)

func TestValidateProfileAcceptsStableIdentityAndKnownKinds(t *testing.T) {
	executable := filepath.Join(t.TempDir(), "terminal")

	for _, kind := range []ProfileKind{ProfileShell, ProfileAgent} {
		t.Run(string(kind), func(t *testing.T) {
			input := Profile{
				ID:         "stable-profile-id",
				Name:       "Stable profile name",
				Kind:       kind,
				Executable: executable,
			}

			first, err := validateProfile(input, failingLookPath)
			if err != nil {
				t.Fatalf("validateProfile: %v", err)
			}
			second, err := validateProfile(input, failingLookPath)
			if err != nil {
				t.Fatalf("validateProfile again: %v", err)
			}
			if first.ID != input.ID || first.Name != input.Name {
				t.Fatalf("identity changed: got ID %q name %q", first.ID, first.Name)
			}
			if !reflect.DeepEqual(first, second) {
				t.Fatalf("validation is not stable:\nfirst:  %#v\nsecond: %#v", first, second)
			}
		})
	}
}

func TestValidateProfileRejectsMissingIdentityAndUnknownKind(t *testing.T) {
	executable := filepath.Join(t.TempDir(), "terminal")
	tests := []struct {
		name   string
		change func(*Profile)
	}{
		{name: "empty ID", change: func(profile *Profile) { profile.ID = "" }},
		{name: "blank ID", change: func(profile *Profile) { profile.ID = " \t" }},
		{name: "empty name", change: func(profile *Profile) { profile.Name = "" }},
		{name: "blank name", change: func(profile *Profile) { profile.Name = "\n" }},
		{name: "empty kind", change: func(profile *Profile) { profile.Kind = "" }},
		{name: "unknown kind", change: func(profile *Profile) { profile.Kind = "task-runner" }},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			profile := Profile{
				ID:         "profile",
				Name:       "Profile",
				Kind:       ProfileShell,
				Executable: executable,
			}
			test.change(&profile)

			if _, err := validateProfile(profile, failingLookPath); err == nil {
				t.Fatal("validateProfile succeeded, want error")
			}
		})
	}
}

func TestValidateProfileAcceptsAbsoluteOrLookPathResolvableExecutable(t *testing.T) {
	absolute := filepath.Join(t.TempDir(), "terminal")
	base := Profile{
		ID:   "profile",
		Name: "Profile",
		Kind: ProfileShell,
	}

	absoluteProfile := base
	absoluteProfile.Executable = absolute
	if _, err := validateProfile(absoluteProfile, func(string) (string, error) {
		t.Fatal("LookPath called for an absolute executable")
		return "", nil
	}); err != nil {
		t.Fatalf("validate absolute executable: %v", err)
	}

	resolvableProfile := base
	resolvableProfile.Executable = "test-shell"
	gotLookup := ""
	if _, err := validateProfile(resolvableProfile, func(name string) (string, error) {
		gotLookup = name
		return absolute, nil
	}); err != nil {
		t.Fatalf("validate resolvable executable: %v", err)
	}
	if gotLookup != resolvableProfile.Executable {
		t.Fatalf("LookPath called with %q, want %q", gotLookup, resolvableProfile.Executable)
	}

	unresolvableProfile := base
	unresolvableProfile.Executable = "missing-shell"
	if _, err := validateProfile(unresolvableProfile, failingLookPath); err == nil {
		t.Fatal("validateProfile accepted an unresolvable executable")
	}
}

func TestValidateProfileRejectsNULValues(t *testing.T) {
	executable := filepath.Join(t.TempDir(), "terminal")
	tests := []struct {
		name   string
		change func(*Profile)
	}{
		{name: "ID", change: func(profile *Profile) { profile.ID = "profile\x00id" }},
		{name: "name", change: func(profile *Profile) { profile.Name = "Pro\x00file" }},
		{name: "executable", change: func(profile *Profile) { profile.Executable = "term\x00inal" }},
		{name: "argument", change: func(profile *Profile) { profile.Args = []string{"--flag", "bad\x00arg"} }},
		{name: "environment key", change: func(profile *Profile) {
			profile.Env = map[string]string{"BAD\x00KEY": "value"}
		}},
		{name: "environment value", change: func(profile *Profile) {
			profile.Env = map[string]string{"KEY": "bad\x00value"}
		}},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			profile := Profile{
				ID:         "profile",
				Name:       "Profile",
				Kind:       ProfileShell,
				Executable: executable,
			}
			test.change(&profile)

			if _, err := validateProfile(profile, failingLookPath); err == nil {
				t.Fatal("validateProfile succeeded, want NUL validation error")
			}
		})
	}
}

func TestValidateProfileReturnsDeepCopyWithoutMutatingCaller(t *testing.T) {
	args := []string{"--mode", "interactive"}
	environment := map[string]string{"AGENT_MODE": "safe"}
	input := Profile{
		ID:         "agent",
		Name:       "Agent",
		Kind:       ProfileAgent,
		Executable: filepath.Join(t.TempDir(), "agent"),
		Args:       args,
		Env:        environment,
	}
	wantInput := Profile{
		ID:         input.ID,
		Name:       input.Name,
		Kind:       input.Kind,
		Executable: input.Executable,
		Args:       append([]string(nil), args...),
		Env:        map[string]string{"AGENT_MODE": "safe"},
	}

	got, err := validateProfile(input, failingLookPath)
	if err != nil {
		t.Fatalf("validateProfile: %v", err)
	}
	if !reflect.DeepEqual(input, wantInput) {
		t.Fatalf("validateProfile mutated input:\ngot:  %#v\nwant: %#v", input, wantInput)
	}

	args[0] = "--changed-by-caller"
	environment["AGENT_MODE"] = "changed-by-caller"
	if got.Args[0] != "--mode" || got.Env["AGENT_MODE"] != "safe" {
		t.Fatalf("validated profile aliases caller data: %#v", got)
	}

	got.Args[1] = "changed-in-result"
	got.Env["AGENT_MODE"] = "changed-in-result"
	if args[1] != "interactive" || environment["AGENT_MODE"] != "changed-by-caller" {
		t.Fatal("mutating validated profile changed caller data")
	}
}

func TestDiscoverProfilesDeterministicallyFindsLoginShellAndInstalledAgents(t *testing.T) {
	shell := filepath.Join(t.TempDir(), "zsh")
	installed := map[string]string{
		"agy":    filepath.Join(t.TempDir(), "agy"),
		"claude": filepath.Join(t.TempDir(), "claude"),
		"codex":  filepath.Join(t.TempDir(), "codex"),
		"gemini": filepath.Join(t.TempDir(), "gemini"),
	}
	dependencies := profileDependencies{
		goos: "darwin",
		getenv: func(name string) string {
			if name == "SHELL" {
				return shell
			}
			return ""
		},
		lookPath: func(name string) (string, error) {
			if path, ok := installed[name]; ok {
				return path, nil
			}
			return "", os.ErrNotExist
		},
	}

	first, err := discoverProfiles(dependencies)
	if err != nil {
		t.Fatalf("discoverProfiles: %v", err)
	}
	second, err := discoverProfiles(dependencies)
	if err != nil {
		t.Fatalf("discoverProfiles again: %v", err)
	}
	if !reflect.DeepEqual(first, second) {
		t.Fatalf("discovery is not deterministic:\nfirst:  %#v\nsecond: %#v", first, second)
	}
	if len(first) != 5 {
		t.Fatalf("got %d profiles, want login shell plus four installed agents: %#v", len(first), first)
	}

	wantExecutables := map[string]ProfileKind{
		shell:               ProfileShell,
		installed["agy"]:    ProfileAgent,
		installed["claude"]: ProfileAgent,
		installed["codex"]:  ProfileAgent,
		installed["gemini"]: ProfileAgent,
	}
	seenIDs := make(map[string]bool)
	for _, profile := range first {
		wantKind, ok := wantExecutables[profile.Executable]
		if !ok {
			t.Fatalf("discovered unavailable or unsupported executable %q", profile.Executable)
		}
		if profile.Kind != wantKind {
			t.Fatalf("profile %q kind = %q, want %q", profile.ID, profile.Kind, wantKind)
		}
		if profile.Kind == ProfileAgent && strings.TrimSpace(profile.Provider) == "" {
			t.Fatalf("agent profile has no provider metadata: %#v", profile)
		}
		if strings.TrimSpace(profile.ID) == "" || strings.TrimSpace(profile.Name) == "" {
			t.Fatalf("discovered profile has empty identity: %#v", profile)
		}
		if seenIDs[profile.ID] {
			t.Fatalf("duplicate discovered profile ID %q", profile.ID)
		}
		seenIDs[profile.ID] = true
		delete(wantExecutables, profile.Executable)
	}
	if len(wantExecutables) != 0 {
		t.Fatalf("missing discovered executables: %#v", wantExecutables)
	}

	shellProfile := profileForExecutable(t, first, shell)
	if !reflect.DeepEqual(shellProfile.Args, []string{"-l"}) {
		t.Fatalf("login shell args = %#v, want []string{\"-l\"}", shellProfile.Args)
	}
}

func TestDiscoverProfilesFindsHomebrewAgentOutsideDesktopPATH(t *testing.T) {
	shell := filepath.Join(t.TempDir(), "zsh")
	gemini := filepath.Join("/opt/homebrew/bin", "gemini")
	dependencies := profileDependencies{
		goos: "darwin",
		getenv: func(name string) string {
			if name == "SHELL" {
				return shell
			}
			return ""
		},
		lookPath: func(name string) (string, error) {
			if name == gemini {
				return gemini, nil
			}
			return "", os.ErrNotExist
		},
	}

	profiles, err := discoverProfiles(dependencies)
	if err != nil {
		t.Fatalf("discoverProfiles: %v", err)
	}
	profile := profileForExecutable(t, profiles, gemini)
	if profile.ID != "agent-gemini" || profile.Provider != "gemini" {
		t.Fatalf("unexpected Gemini profile: %#v", profile)
	}
}

func TestDiscoverProfilesUsesWindowsCommandProcessorWithoutUnixLoginFlag(t *testing.T) {
	commandProcessor := filepath.Join(t.TempDir(), "cmd.exe")
	dependencies := profileDependencies{
		goos: "windows",
		getenv: func(name string) string {
			if name == "COMSPEC" {
				return commandProcessor
			}
			return ""
		},
		lookPath: failingLookPath,
	}

	profiles, err := discoverProfiles(dependencies)
	if err != nil {
		t.Fatalf("discoverProfiles: %v", err)
	}
	if len(profiles) != 1 {
		t.Fatalf("got %d profiles, want only command processor: %#v", len(profiles), profiles)
	}
	if profiles[0].Executable != commandProcessor || profiles[0].Kind != ProfileShell {
		t.Fatalf("unexpected command processor profile: %#v", profiles[0])
	}
	if len(profiles[0].Args) != 0 {
		t.Fatalf("Windows command processor args = %#v, want none", profiles[0].Args)
	}
}

func TestBuildEnvironmentAppliesTerminalDefaults(t *testing.T) {
	base := []string{
		"PATH=/usr/bin",
		"NO_COLOR=1",
		"TERM=vt100",
		"COLORTERM=",
		"TERM_PROGRAM=another-terminal",
		"UNCHANGED=value",
	}
	wantBase := append([]string(nil), base...)

	got, err := buildEnvironment(base, nil)
	if err != nil {
		t.Fatalf("buildEnvironment: %v", err)
	}
	values := environmentValues(t, got)
	want := map[string]string{
		"PATH":         "/usr/bin",
		"TERM":         "xterm-256color",
		"COLORTERM":    "truecolor",
		"TERM_PROGRAM": "p-track",
		"UNCHANGED":    "value",
	}
	if locale := defaultUTF8Locale(runtime.GOOS); locale != "" {
		want["LANG"] = locale
	}
	if !reflect.DeepEqual(values, want) {
		t.Fatalf("environment values:\ngot:  %#v\nwant: %#v", values, want)
	}
	if !reflect.DeepEqual(base, wantBase) {
		t.Fatalf("buildEnvironment mutated base:\ngot:  %#v\nwant: %#v", base, wantBase)
	}
}

func TestBuildEnvironmentDropsInheritedColorSuppressionButAllowsOverride(t *testing.T) {
	got, err := buildEnvironmentForOS([]string{"NO_COLOR=1"}, nil, "darwin")
	if err != nil {
		t.Fatalf("buildEnvironmentForOS: %v", err)
	}
	if _, ok := environmentValues(t, got)["NO_COLOR"]; ok {
		t.Fatal("inherited NO_COLOR leaked into interactive terminal")
	}

	got, err = buildEnvironmentForOS(nil, map[string]string{"NO_COLOR": "1"}, "darwin")
	if err != nil {
		t.Fatalf("buildEnvironmentForOS with override: %v", err)
	}
	if value := environmentValues(t, got)["NO_COLOR"]; value != "1" {
		t.Fatalf("explicit NO_COLOR = %q, want 1", value)
	}
}

func TestBuildEnvironmentProvidesUTF8LocaleWhenDesktopEnvironmentHasNone(t *testing.T) {
	tests := []struct {
		goos string
		want string
	}{
		{goos: "darwin", want: "en_US.UTF-8"},
		{goos: "linux", want: "C.UTF-8"},
	}
	for _, test := range tests {
		t.Run(test.goos, func(t *testing.T) {
			got, err := buildEnvironmentForOS([]string{"PATH=/usr/bin"}, nil, test.goos)
			if err != nil {
				t.Fatalf("buildEnvironmentForOS: %v", err)
			}
			if locale := environmentValues(t, got)["LANG"]; locale != test.want {
				t.Fatalf("LANG = %q, want %q", locale, test.want)
			}
		})
	}
}

func TestBuildEnvironmentPreservesInheritedOrExplicitLocale(t *testing.T) {
	tests := []struct {
		name      string
		base      []string
		overrides map[string]string
		wantKey   string
		wantValue string
	}{
		{
			name:      "inherited character locale",
			base:      []string{"LC_CTYPE=ja_JP.UTF-8"},
			wantKey:   "LC_CTYPE",
			wantValue: "ja_JP.UTF-8",
		},
		{
			name:      "profile language override",
			overrides: map[string]string{"LANG": "fr_FR.UTF-8"},
			wantKey:   "LANG",
			wantValue: "fr_FR.UTF-8",
		},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			got, err := buildEnvironmentForOS(test.base, test.overrides, "darwin")
			if err != nil {
				t.Fatalf("buildEnvironmentForOS: %v", err)
			}
			values := environmentValues(t, got)
			if values[test.wantKey] != test.wantValue {
				t.Fatalf("%s = %q, want %q", test.wantKey, values[test.wantKey], test.wantValue)
			}
			if test.wantKey != "LANG" {
				if _, ok := values["LANG"]; ok {
					t.Fatalf("unexpected fallback LANG alongside %s: %#v", test.wantKey, values)
				}
			}
		})
	}
}

func TestBuildEnvironmentAppliesSafeExplicitOverridesWithoutMutation(t *testing.T) {
	base := []string{"PATH=/usr/bin", "BASE=value"}
	overrides := map[string]string{
		"TERM":         "screen-256color",
		"COLORTERM":    "24bit",
		"TERM_PROGRAM": "p-track-test",
		"CUSTOM":       "custom value",
	}
	wantBase := append([]string(nil), base...)
	wantOverrides := map[string]string{
		"TERM":         "screen-256color",
		"COLORTERM":    "24bit",
		"TERM_PROGRAM": "p-track-test",
		"CUSTOM":       "custom value",
	}

	got, err := buildEnvironment(base, overrides)
	if err != nil {
		t.Fatalf("buildEnvironment: %v", err)
	}
	values := environmentValues(t, got)
	for key, want := range wantOverrides {
		if values[key] != want {
			t.Errorf("%s = %q, want explicit override %q", key, values[key], want)
		}
	}
	if !reflect.DeepEqual(base, wantBase) {
		t.Fatalf("buildEnvironment mutated base:\ngot:  %#v\nwant: %#v", base, wantBase)
	}
	if !reflect.DeepEqual(overrides, wantOverrides) {
		t.Fatalf("buildEnvironment mutated overrides:\ngot:  %#v\nwant: %#v", overrides, wantOverrides)
	}
}

func TestBuildEnvironmentRejectsUnsafeOverrides(t *testing.T) {
	tests := []map[string]string{
		{"BAD=KEY": "value"},
		{"=C:": `C:\unsafe`},
		{"BAD\x00KEY": "value"},
		{"KEY": "bad\x00value"},
	}
	for _, overrides := range tests {
		if _, err := buildEnvironment(nil, overrides); err == nil {
			t.Fatalf("buildEnvironment accepted unsafe overrides %#v", overrides)
		}
	}
}

func TestBuildEnvironmentHandlesWindowsKeysAndDriveDirectoryEntries(t *testing.T) {
	got, err := buildEnvironmentForOS([]string{
		`=C:=C:\work`,
		`Path=C:\Windows`,
		"term=vt100",
	}, map[string]string{
		"PATH": `C:\Tools`,
		"TERM": "screen-256color",
	}, "windows")
	if err != nil {
		t.Fatalf("buildEnvironmentForOS: %v", err)
	}

	values := environmentValues(t, got)
	want := map[string]string{
		"=C:":          `C:\work`,
		"PATH":         `C:\Tools`,
		"TERM":         "screen-256color",
		"COLORTERM":    "truecolor",
		"TERM_PROGRAM": "p-track",
	}
	if !reflect.DeepEqual(values, want) {
		t.Fatalf("environment values:\ngot:  %#v\nwant: %#v", values, want)
	}
}

func TestResolveCWDDefaultsToProjectRootAndAcceptsDirectory(t *testing.T) {
	projectRoot := t.TempDir()

	got, err := resolveCWD(projectRoot, "")
	if err != nil {
		t.Fatalf("resolve default CWD: %v", err)
	}
	if got != projectRoot {
		t.Fatalf("default CWD = %q, want project root %q", got, projectRoot)
	}

	requested := t.TempDir()
	got, err = resolveCWD(projectRoot, requested)
	if err != nil {
		t.Fatalf("resolve requested CWD: %v", err)
	}
	if got != requested {
		t.Fatalf("requested CWD = %q, want %q", got, requested)
	}
}

func TestResolveCWDRejectsMissingAndNonDirectoryPaths(t *testing.T) {
	projectRoot := t.TempDir()
	missing := filepath.Join(t.TempDir(), "missing")
	if _, err := resolveCWD(projectRoot, missing); err == nil {
		t.Fatal("resolveCWD accepted a missing path")
	}

	file := filepath.Join(t.TempDir(), "file")
	if err := os.WriteFile(file, []byte("not a directory"), 0o600); err != nil {
		t.Fatalf("write test file: %v", err)
	}
	if _, err := resolveCWD(projectRoot, file); err == nil {
		t.Fatal("resolveCWD accepted a non-directory path")
	}

	missingRoot := filepath.Join(t.TempDir(), "missing-root")
	if _, err := resolveCWD(missingRoot, ""); err == nil {
		t.Fatal("resolveCWD accepted a missing default project root")
	}
}

func failingLookPath(string) (string, error) {
	return "", errors.New("executable not found")
}

func TestDiscoverProfilesUsesAccountShellWhenEnvMissing(t *testing.T) {
	recorded := filepath.Join(t.TempDir(), "zsh")
	dependencies := profileDependencies{
		goos:      "darwin",
		getenv:    func(string) string { return "" },
		lookPath:  failingLookPath,
		userShell: func() (string, error) { return recorded, nil },
	}

	profiles, err := discoverProfiles(dependencies)
	if err != nil {
		t.Fatalf("discoverProfiles: %v", err)
	}
	shell := profileForExecutable(t, profiles, recorded)
	if shell.Kind != ProfileShell {
		t.Fatalf("account shell profile kind = %q, want %q", shell.Kind, ProfileShell)
	}
	if !reflect.DeepEqual(shell.Args, []string{"-l"}) {
		t.Fatalf("account shell args = %#v, want login shell", shell.Args)
	}
}

func TestDiscoverProfilesAccountShellWinsOverInheritedEnv(t *testing.T) {
	recorded := filepath.Join(t.TempDir(), "zsh")
	inherited := filepath.Join(t.TempDir(), "bash")
	dependencies := profileDependencies{
		goos: "darwin",
		getenv: func(name string) string {
			if name == "SHELL" {
				return inherited
			}
			return ""
		},
		lookPath:  failingLookPath,
		userShell: func() (string, error) { return recorded, nil },
	}

	profiles, err := discoverProfiles(dependencies)
	if err != nil {
		t.Fatalf("discoverProfiles: %v", err)
	}
	profileForExecutable(t, profiles, recorded)
}

func TestDiscoverProfilesPrefersZshOverShWhenAccountLookupFails(t *testing.T) {
	zsh := filepath.Join(t.TempDir(), "zsh")
	dependencies := profileDependencies{
		goos:   "darwin",
		getenv: func(string) string { return "" },
		lookPath: func(name string) (string, error) {
			if name == "zsh" {
				return zsh, nil
			}
			return "", errors.New("executable not found")
		},
		userShell: func() (string, error) { return "", errors.New("no directory services") },
	}

	profiles, err := discoverProfiles(dependencies)
	if err != nil {
		t.Fatalf("discoverProfiles: %v", err)
	}
	profileForExecutable(t, profiles, zsh)
}

func TestDiscoverProfilesStillFallsBackToSh(t *testing.T) {
	sh := filepath.Join(t.TempDir(), "sh")
	dependencies := profileDependencies{
		goos:   "darwin",
		getenv: func(string) string { return "" },
		lookPath: func(name string) (string, error) {
			if name == "sh" {
				return sh, nil
			}
			return "", errors.New("executable not found")
		},
		userShell: func() (string, error) { return "", errors.New("no directory services") },
	}

	profiles, err := discoverProfiles(dependencies)
	if err != nil {
		t.Fatalf("discoverProfiles: %v", err)
	}
	profileForExecutable(t, profiles, sh)
}

func profileForExecutable(t *testing.T, profiles []Profile, executable string) Profile {
	t.Helper()
	for _, profile := range profiles {
		if profile.Executable == executable {
			return profile
		}
	}
	t.Fatalf("profile for executable %q not found in %#v", executable, profiles)
	return Profile{}
}

func environmentValues(t *testing.T, environment []string) map[string]string {
	t.Helper()
	values := make(map[string]string, len(environment))
	for _, entry := range environment {
		key, value, ok := splitInheritedEnvironment(entry, "windows")
		if !ok {
			t.Fatalf("invalid environment entry %q", entry)
		}
		values[key] = value
	}
	return values
}
