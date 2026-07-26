package terminal

import (
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"runtime"
	"sort"
	"strings"
)

type ProfileKind string

const (
	ProfileShell ProfileKind = "shell"
	ProfileAgent ProfileKind = "agent"
)

type Profile struct {
	ID         string            `json:"id"`
	Name       string            `json:"name"`
	Kind       ProfileKind       `json:"kind"`
	Provider   string            `json:"provider,omitempty"`
	Executable string            `json:"executable"`
	Args       []string          `json:"args"`
	Env        map[string]string `json:"env"`
}

var stableProfileID = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9._-]*$`)

type profileDependencies struct {
	lookPath func(string) (string, error)
	getenv   func(string) string
	goos     string
}

type agentCandidate struct {
	id         string
	name       string
	provider   string
	executable string
}

var supportedAgentCandidates = []agentCandidate{
	{id: "agent-claude", name: "Claude Code", provider: "claude", executable: "claude"},
	{id: "agent-codex", name: "Codex", provider: "codex", executable: "codex"},
	{id: "agent-gemini", name: "Gemini", provider: "gemini", executable: "gemini"},
	{id: "agent-opencode", name: "OpenCode", provider: "opencode", executable: "opencode"},
}

func ValidateProfile(profile Profile) (Profile, error) {
	return validateProfile(profile, exec.LookPath)
}

func validateProfile(profile Profile, lookPath func(string) (string, error)) (Profile, error) {
	if !stableProfileID.MatchString(profile.ID) {
		return Profile{}, errors.New("profile ID must be stable and nonempty")
	}
	if strings.TrimSpace(profile.Name) == "" {
		return Profile{}, errors.New("profile name must be nonempty")
	}
	if profile.Kind != ProfileShell && profile.Kind != ProfileAgent {
		return Profile{}, fmt.Errorf("unknown profile kind %q", profile.Kind)
	}
	if profile.Kind == ProfileAgent && strings.TrimSpace(profile.Provider) == "" {
		profile.Provider = strings.TrimPrefix(profile.ID, "agent-")
	}
	if profile.Kind == ProfileAgent && strings.TrimSpace(profile.Provider) == "" {
		return Profile{}, errors.New("agent profile provider must be nonempty")
	}
	if strings.TrimSpace(profile.Executable) == "" {
		return Profile{}, errors.New("profile executable must be nonempty")
	}
	if containsNUL(profile.ID) || containsNUL(profile.Name) ||
		containsNUL(profile.Provider) || containsNUL(profile.Executable) {
		return Profile{}, errors.New("profile contains a NUL value")
	}

	clone := cloneProfile(profile)
	for _, argument := range clone.Args {
		if containsNUL(argument) {
			return Profile{}, errors.New("profile argument contains NUL")
		}
	}
	for key, value := range clone.Env {
		if !safeEnvironmentEntry(key, value) {
			return Profile{}, fmt.Errorf("profile environment override %q is unsafe", key)
		}
	}

	if !filepath.IsAbs(clone.Executable) {
		resolved, err := lookPath(clone.Executable)
		if err != nil {
			return Profile{}, fmt.Errorf("resolve profile executable %q: %w", clone.Executable, err)
		}
		if !filepath.IsAbs(resolved) {
			resolved, err = filepath.Abs(resolved)
			if err != nil {
				return Profile{}, fmt.Errorf("make profile executable absolute: %w", err)
			}
		}
		clone.Executable = resolved
	}
	return clone, nil
}

func DiscoverProfiles() ([]Profile, error) {
	return discoverProfiles(profileDependencies{
		lookPath: exec.LookPath,
		getenv:   os.Getenv,
		goos:     runtime.GOOS,
	})
}

func discoverProfiles(dependencies profileDependencies) ([]Profile, error) {
	if dependencies.lookPath == nil || dependencies.getenv == nil {
		return nil, errors.New("profile discovery dependencies are incomplete")
	}

	shellExecutable, shellArgs, err := discoverDefaultShell(dependencies)
	if err != nil {
		return nil, err
	}
	shell, err := validateProfile(Profile{
		ID:         "shell-default",
		Name:       "Default shell",
		Kind:       ProfileShell,
		Executable: shellExecutable,
		Args:       shellArgs,
	}, dependencies.lookPath)
	if err != nil {
		return nil, fmt.Errorf("validate default shell: %w", err)
	}

	profiles := []Profile{shell}
	for _, candidate := range supportedAgentCandidates {
		executable, lookupErr := dependencies.lookPath(candidate.executable)
		if lookupErr != nil {
			continue
		}
		profile, validateErr := validateProfile(Profile{
			ID:         candidate.id,
			Name:       candidate.name,
			Kind:       ProfileAgent,
			Provider:   candidate.provider,
			Executable: executable,
		}, dependencies.lookPath)
		if validateErr != nil {
			return nil, fmt.Errorf("validate discovered %s profile: %w", candidate.name, validateErr)
		}
		profiles = append(profiles, profile)
	}
	return profiles, nil
}

func discoverDefaultShell(dependencies profileDependencies) (string, []string, error) {
	if dependencies.goos == "windows" {
		executable := dependencies.getenv("COMSPEC")
		if executable == "" {
			var err error
			executable, err = dependencies.lookPath("cmd.exe")
			if err != nil {
				return "", nil, errors.New("default Windows command processor not found")
			}
		}
		return executable, nil, nil
	}

	executable := dependencies.getenv("SHELL")
	if executable == "" {
		var err error
		executable, err = dependencies.lookPath("sh")
		if err != nil {
			return "", nil, errors.New("default shell not found")
		}
	}
	return executable, []string{"-l"}, nil
}

func buildEnvironment(base []string, overrides map[string]string) ([]string, error) {
	return buildEnvironmentForOS(base, overrides, runtime.GOOS)
}

func buildEnvironmentForOS(base []string, overrides map[string]string, goos string) ([]string, error) {
	type environmentValue struct {
		key   string
		value string
	}
	values := make(map[string]environmentValue, len(base)+len(overrides)+3)
	normalize := func(key string) string {
		if goos == "windows" {
			return strings.ToUpper(key)
		}
		return key
	}
	set := func(key, value string) {
		values[normalize(key)] = environmentValue{key: key, value: value}
	}

	for _, entry := range base {
		key, value, ok := splitInheritedEnvironment(entry, goos)
		if !ok || containsNUL(key) || containsNUL(value) {
			return nil, fmt.Errorf("invalid inherited environment entry %q", key)
		}
		set(key, value)
	}

	set("TERM", "xterm-256color")
	set("COLORTERM", "truecolor")
	set("TERM_PROGRAM", "P-TRACK")
	for key, value := range overrides {
		if !safeEnvironmentEntry(key, value) {
			return nil, fmt.Errorf("unsafe environment override %q", key)
		}
		set(key, value)
	}

	keys := make([]string, 0, len(values))
	for key := range values {
		keys = append(keys, key)
	}
	sort.Strings(keys)
	environment := make([]string, 0, len(keys))
	for _, key := range keys {
		value := values[key]
		environment = append(environment, value.key+"="+value.value)
	}
	return environment, nil
}

func splitInheritedEnvironment(entry, goos string) (string, string, bool) {
	if goos == "windows" && strings.HasPrefix(entry, "=") {
		separator := strings.Index(entry[1:], "=")
		if separator < 0 {
			return "", "", false
		}
		separator++
		return entry[:separator], entry[separator+1:], true
	}
	key, value, ok := strings.Cut(entry, "=")
	return key, value, ok && key != ""
}

func resolveCWD(projectRoot, requested string) (string, error) {
	cwd := requested
	if cwd == "" {
		cwd = projectRoot
	}
	absolute, err := filepath.Abs(cwd)
	if err != nil {
		return "", fmt.Errorf("resolve working directory: %w", err)
	}
	info, err := os.Stat(absolute)
	if err != nil {
		return "", fmt.Errorf("stat working directory: %w", err)
	}
	if !info.IsDir() {
		return "", fmt.Errorf("working directory %q is not a directory", absolute)
	}
	return filepath.Clean(absolute), nil
}

func cloneProfile(profile Profile) Profile {
	profile.Args = append([]string(nil), profile.Args...)
	if profile.Env != nil {
		environment := make(map[string]string, len(profile.Env))
		for key, value := range profile.Env {
			environment[key] = value
		}
		profile.Env = environment
	}
	return profile
}

func safeEnvironmentEntry(key, value string) bool {
	return key != "" && !strings.ContainsAny(key, "=\x00") && !containsNUL(value)
}

func containsNUL(value string) bool {
	return strings.ContainsRune(value, '\x00')
}
