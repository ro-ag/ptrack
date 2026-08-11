package terminal

import (
	"errors"
	"fmt"
	"os"
	"os/exec"
	"os/user"
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

type CWDPolicy string

const (
	// CWDRequested accepts a caller-selected working directory and otherwise
	// starts at the project root. This preserves the pre-profile-config
	// behavior and is the safe default for older profiles.
	CWDRequested CWDPolicy = "requested"
	// CWDProject always starts at the project root.
	CWDProject CWDPolicy = "project"
	// CWDFixed always starts at the profile's validated FixedCWD.
	CWDFixed CWDPolicy = "fixed"
)

type ExitBehavior string

const (
	// ExitKeep leaves the stopped terminal visible after its direct process
	// exits. Automatic process restart is intentionally not a profile option.
	ExitKeep           ExitBehavior = "keep"
	ExitCloseOnSuccess ExitBehavior = "close-on-success"
	ExitClose          ExitBehavior = "close"
)

const (
	DefaultProfileTheme      = "default"
	DefaultProfileFontFamily = "monospace"
	DefaultProfileFontSize   = 14
	DefaultProfileScrollback = 25_000

	MinProfileFontSize   = 10
	MaxProfileFontSize   = 24
	MinProfileScrollback = 100
	MaxProfileScrollback = 100_000

	maxProfileIDBytes          = 128
	maxProfileNameBytes        = 256
	maxProfileProviderBytes    = 128
	maxProfileExecutableBytes  = 4_096
	maxProfileArgumentCount    = 64
	maxProfileArgumentBytes    = 4_096
	maxProfileArgumentsBytes   = 64 * 1_024
	maxProfileEnvironmentCount = 64
	maxProfileEnvironmentKey   = 128
	maxProfileEnvironmentValue = 4_096
	maxProfileEnvironmentBytes = 64 * 1_024
	maxProfileThemeBytes       = 64
	maxProfileFontFamilyBytes  = 256
	maxProfileCWDBytes         = 4_096
)

type Profile struct {
	ID           string            `json:"id"`
	Name         string            `json:"name"`
	Kind         ProfileKind       `json:"kind"`
	Provider     string            `json:"provider,omitempty"`
	Executable   string            `json:"executable"`
	Args         []string          `json:"args"`
	Env          map[string]string `json:"env"`
	Theme        string            `json:"theme"`
	FontFamily   string            `json:"fontFamily"`
	FontSize     int               `json:"fontSize"`
	Scrollback   int               `json:"scrollback"`
	CWDPolicy    CWDPolicy         `json:"cwdPolicy"`
	FixedCWD     string            `json:"fixedCwd,omitempty"`
	ExitBehavior ExitBehavior      `json:"exitBehavior"`
}

// SortProfiles puts the safe default shell first, followed by other shells
// and then agents. IDs and names make discovery and map-backed manager results
// deterministic across processes.
func SortProfiles(profiles []Profile) {
	rank := func(profile Profile) int {
		if profile.Kind == ProfileShell && profile.ID == "shell-default" {
			return 0
		}
		if profile.Kind == ProfileShell {
			return 1
		}
		return 2
	}
	sort.Slice(profiles, func(left, right int) bool {
		leftRank, rightRank := rank(profiles[left]), rank(profiles[right])
		if leftRank != rightRank {
			return leftRank < rightRank
		}
		if profiles[left].ID != profiles[right].ID {
			return profiles[left].ID < profiles[right].ID
		}
		if profiles[left].Name != profiles[right].Name {
			return profiles[left].Name < profiles[right].Name
		}
		return profiles[left].Provider < profiles[right].Provider
	})
}

var stableProfileID = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9._-]*$`)

type profileDependencies struct {
	lookPath func(string) (string, error)
	getenv   func(string) string
	goos     string
	// userShell resolves the login shell from the OS account record. Optional;
	// used when SHELL is absent from the environment (Finder-launched apps).
	userShell func() (string, error)
}

type agentCandidate struct {
	id         string
	name       string
	provider   string
	executable string
}

var supportedAgentCandidates = []agentCandidate{
	{id: "agent-agy", name: "Agy", provider: "agy", executable: "agy"},
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
	if len(profile.ID) > maxProfileIDBytes {
		return Profile{}, errors.New("profile ID is too long")
	}
	if strings.TrimSpace(profile.Name) == "" {
		return Profile{}, errors.New("profile name must be nonempty")
	}
	if len(profile.Name) > maxProfileNameBytes {
		return Profile{}, errors.New("profile name is too long")
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
	if len(profile.Provider) > maxProfileProviderBytes {
		return Profile{}, errors.New("profile provider is too long")
	}
	if strings.TrimSpace(profile.Executable) == "" {
		return Profile{}, errors.New("profile executable must be nonempty")
	}
	if len(profile.Executable) > maxProfileExecutableBytes {
		return Profile{}, errors.New("profile executable is too long")
	}
	if containsNUL(profile.ID) || containsNUL(profile.Name) ||
		containsNUL(profile.Provider) || containsNUL(profile.Executable) {
		return Profile{}, errors.New("profile contains a NUL value")
	}

	clone := cloneProfile(profile)
	if len(clone.Args) > maxProfileArgumentCount {
		return Profile{}, errors.New("profile has too many arguments")
	}
	argumentBytes := 0
	for _, argument := range clone.Args {
		if containsNUL(argument) {
			return Profile{}, errors.New("profile argument contains NUL")
		}
		if len(argument) > maxProfileArgumentBytes {
			return Profile{}, errors.New("profile argument is too long")
		}
		argumentBytes += len(argument)
	}
	if argumentBytes > maxProfileArgumentsBytes {
		return Profile{}, errors.New("profile arguments are too large")
	}
	if len(clone.Env) > maxProfileEnvironmentCount {
		return Profile{}, errors.New("profile has too many environment overrides")
	}
	environmentBytes := 0
	for key, value := range clone.Env {
		if !safeProfileEnvironmentEntry(key, value) {
			return Profile{}, fmt.Errorf("profile environment override %q is unsafe", key)
		}
		environmentBytes += len(key) + len(value)
	}
	if environmentBytes > maxProfileEnvironmentBytes {
		return Profile{}, errors.New("profile environment overrides are too large")
	}
	if err := normalizeProfilePresentation(&clone); err != nil {
		return Profile{}, err
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

func normalizeProfilePresentation(profile *Profile) error {
	if profile.Theme == "" {
		profile.Theme = DefaultProfileTheme
	}
	if len(profile.Theme) > maxProfileThemeBytes ||
		!stableProfileID.MatchString(profile.Theme) {
		return errors.New("profile theme must be a bounded stable name")
	}
	if profile.FontFamily == "" {
		profile.FontFamily = DefaultProfileFontFamily
	}
	if len(profile.FontFamily) > maxProfileFontFamilyBytes ||
		containsNUL(profile.FontFamily) || strings.TrimSpace(profile.FontFamily) == "" {
		return errors.New("profile font family is invalid")
	}
	if profile.FontSize == 0 {
		profile.FontSize = DefaultProfileFontSize
	}
	if profile.FontSize < MinProfileFontSize || profile.FontSize > MaxProfileFontSize {
		return fmt.Errorf("profile font size must be between %d and %d", MinProfileFontSize, MaxProfileFontSize)
	}
	if profile.Scrollback == 0 {
		profile.Scrollback = DefaultProfileScrollback
	}
	if profile.Scrollback < MinProfileScrollback || profile.Scrollback > MaxProfileScrollback {
		return fmt.Errorf("profile scrollback must be between %d and %d", MinProfileScrollback, MaxProfileScrollback)
	}
	if profile.CWDPolicy == "" {
		profile.CWDPolicy = CWDRequested
	}
	switch profile.CWDPolicy {
	case CWDRequested, CWDProject:
		if profile.FixedCWD != "" {
			return errors.New("profile fixed working directory requires fixed policy")
		}
	case CWDFixed:
		if profile.FixedCWD == "" {
			return errors.New("fixed working-directory policy requires a path")
		}
		if len(profile.FixedCWD) > maxProfileCWDBytes || containsNUL(profile.FixedCWD) ||
			!filepath.IsAbs(profile.FixedCWD) {
			return errors.New("profile fixed working directory must be a bounded absolute path")
		}
		profile.FixedCWD = filepath.Clean(profile.FixedCWD)
	default:
		return fmt.Errorf("unknown profile working-directory policy %q", profile.CWDPolicy)
	}
	if profile.ExitBehavior == "" {
		profile.ExitBehavior = ExitKeep
	}
	switch profile.ExitBehavior {
	case ExitKeep, ExitCloseOnSuccess, ExitClose:
	default:
		return fmt.Errorf("unknown profile exit behavior %q", profile.ExitBehavior)
	}
	return nil
}

func DiscoverProfiles() ([]Profile, error) {
	return discoverProfiles(profileDependencies{
		lookPath:  exec.LookPath,
		getenv:    os.Getenv,
		goos:      runtime.GOOS,
		userShell: directoryServicesUserShell,
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
		executable, lookupErr := discoverAgentExecutable(candidate.executable, dependencies)
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

func discoverAgentExecutable(name string, dependencies profileDependencies) (string, error) {
	executable, err := dependencies.lookPath(name)
	if err == nil {
		return executable, nil
	}
	if dependencies.goos != "darwin" {
		return "", err
	}

	candidates := []string{
		filepath.Join("/opt/homebrew/bin", name),
		filepath.Join("/usr/local/bin", name),
	}
	if home := dependencies.getenv("HOME"); home != "" {
		candidates = append(candidates,
			filepath.Join(home, ".local", "bin", name),
			filepath.Join(home, ".opencode", "bin", name),
		)
	}
	for _, candidate := range candidates {
		if executable, lookupErr := dependencies.lookPath(candidate); lookupErr == nil {
			return executable, nil
		}
	}
	return "", err
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

	executable := ""
	if dependencies.userShell != nil {
		// The account record is the authoritative source (same one
		// Terminal.app uses). $SHELL is not trustworthy here: apps launched
		// through LaunchServices inherit the *requesting* process's
		// environment, so a ptrack started via `open` from a bash session
		// would see SHELL=/bin/bash even for a zsh user.
		if resolved, err := dependencies.userShell(); err == nil {
			executable = resolved
		}
	}
	if executable == "" {
		executable = dependencies.getenv("SHELL")
	}
	if executable == "" && dependencies.goos == "darwin" {
		// zsh has been the macOS default shell since Catalina.
		if resolved, err := dependencies.lookPath("zsh"); err == nil {
			executable = resolved
		}
	}
	if executable == "" {
		var err error
		executable, err = dependencies.lookPath("sh")
		if err != nil {
			return "", nil, errors.New("default shell not found")
		}
	}
	return executable, []string{"-l"}, nil
}

// directoryServicesUserShell reports the current user's login shell as
// registered in Directory Services (e.g. "/bin/zsh").
func directoryServicesUserShell() (string, error) {
	current, err := user.Current()
	if err != nil {
		return "", fmt.Errorf("resolve current user: %w", err)
	}
	output, err := exec.Command("dscl", ".", "-read", "/Users/"+current.Username, "UserShell").Output()
	if err != nil {
		return "", fmt.Errorf("read UserShell from Directory Services: %w", err)
	}
	_, shell, ok := strings.Cut(strings.TrimSpace(string(output)), "UserShell: ")
	if !ok || strings.TrimSpace(shell) == "" {
		return "", errors.New("UserShell not present in Directory Services record")
	}
	return strings.TrimSpace(shell), nil
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
		// A p-track process may itself have been launched by an agent wrapper.
		// Never forward inherited host authority into a new terminal. PTRACK_HOME
		// is configuration, not authority, and remains available to nested CLI
		// commands. Fresh per-launch values are layered below by the host.
		upperKey := strings.ToUpper(key)
		if strings.HasPrefix(upperKey, "PTRACK_") && upperKey != "PTRACK_HOME" {
			continue
		}
		set(key, value)
	}
	// Desktop launchers and parent automation tools may set NO_COLOR for their
	// own logs. Do not leak that policy into an interactive PTY. A profile can
	// still opt out explicitly through its environment overrides below.
	delete(values, normalize("NO_COLOR"))

	set("TERM", "xterm-256color")
	set("COLORTERM", "truecolor")
	set("TERM_PROGRAM", "p-track")
	for key, value := range overrides {
		if !safeEnvironmentEntry(key, value) {
			return nil, fmt.Errorf("unsafe environment override %q", key)
		}
		set(key, value)
	}
	if locale := defaultUTF8Locale(goos); locale != "" {
		hasLocale := false
		for _, key := range []string{"LC_ALL", "LC_CTYPE", "LANG"} {
			if value, ok := values[normalize(key)]; ok && value.value != "" {
				hasLocale = true
				break
			}
		}
		if !hasLocale {
			set("LANG", locale)
		}
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

func defaultUTF8Locale(goos string) string {
	switch goos {
	case "darwin":
		return "en_US.UTF-8"
	case "windows":
		return ""
	default:
		return "C.UTF-8"
	}
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

// ResolveCWD validates and canonicalizes a terminal working directory without
// creating a session or reading terminal runtime state.
func ResolveCWD(projectRoot, requested string) (string, error) {
	return resolveCWD(projectRoot, requested)
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

func safeProfileEnvironmentEntry(key, value string) bool {
	if !safeEnvironmentEntry(key, value) ||
		len(key) > maxProfileEnvironmentKey || len(value) > maxProfileEnvironmentValue {
		return false
	}
	upper := strings.ToUpper(key)
	if strings.HasPrefix(upper, "PTRACK_") {
		return false
	}
	// Persisted profile overrides are intentionally unsuitable for credentials.
	// Host-minted per-launch authority takes a separate path through Manager and
	// is never copied into Profile or profile configuration JSON.
	for _, marker := range []string{
		"TOKEN",
		"SECRET",
		"PASSWORD",
		"PASSWD",
		"API_KEY",
		"APIKEY",
		"PRIVATE_KEY",
		"PRIVATEKEY",
		"ACCESS_KEY",
		"SESSION_KEY",
		"CREDENTIAL",
	} {
		if strings.Contains(upper, marker) {
			return false
		}
	}
	return true
}

func containsNUL(value string) bool {
	return strings.ContainsRune(value, '\x00')
}
