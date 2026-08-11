package terminal

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"maps"
	"os"
	"path/filepath"
	"slices"
)

const (
	ProfileConfigVersion      = 1
	MaxConfiguredProfiles     = 64
	maxProfileConfigJSONBytes = 256 * 1_024
)

// ProfileConfig is the complete, versioned set of explicit profile
// configuration. It never includes the process environment inherited by the
// p-track host; Profile.Env contains only validated, user-authored overrides.
type ProfileConfig struct {
	Version  int       `json:"version"`
	Profiles []Profile `json:"profiles"`
}

// ValidateProfileConfig validates, normalizes, and deep-copies persisted
// configuration without mutating its caller.
func ValidateProfileConfig(config ProfileConfig) (ProfileConfig, error) {
	if config.Version != ProfileConfigVersion {
		return ProfileConfig{}, fmt.Errorf("unsupported terminal profile config version %d", config.Version)
	}
	if len(config.Profiles) > MaxConfiguredProfiles {
		return ProfileConfig{}, errors.New("terminal profile config has too many profiles")
	}
	normalized, err := normalizeProfileSet(config.Profiles, "configured")
	if err != nil {
		return ProfileConfig{}, err
	}
	return ProfileConfig{Version: ProfileConfigVersion, Profiles: normalized}, nil
}

// MergeProfiles returns a deterministic, independently owned profile set.
// Configured profiles replace discovered profiles with the same stable ID and
// otherwise append custom shell profiles. Agent IDs remain host-discovered and
// every launch-affecting field stays immutable so presentation overrides cannot
// silently inherit another process's capability identity.
func MergeProfiles(discovered, configured []Profile) ([]Profile, error) {
	builtins, err := normalizeProfileSet(discovered, "discovered")
	if err != nil {
		return nil, err
	}
	overrides, err := normalizeProfileSet(configured, "configured")
	if err != nil {
		return nil, err
	}
	if len(overrides) > MaxConfiguredProfiles {
		return nil, errors.New("terminal profile config has too many profiles")
	}

	byID := make(map[string]Profile, len(builtins)+len(overrides))
	for _, profile := range builtins {
		byID[profile.ID] = cloneProfile(profile)
	}
	for _, profile := range overrides {
		existing, discoveredID := byID[profile.ID]
		if !discoveredID {
			if profile.Kind == ProfileAgent {
				return nil, fmt.Errorf("configured custom agent profile %q is not allowed", profile.ID)
			}
			byID[profile.ID] = cloneProfile(profile)
			continue
		}
		if profile.Kind != existing.Kind {
			return nil, fmt.Errorf("configured terminal profile %q changes discovered kind", profile.ID)
		}
		if profile.Provider != existing.Provider {
			return nil, fmt.Errorf("configured terminal profile %q changes discovered provider", profile.ID)
		}
		if profile.Kind == ProfileAgent && !sameAgentLaunchIdentity(profile, existing) {
			return nil, fmt.Errorf("configured agent profile %q changes discovered launch identity", profile.ID)
		}
		byID[profile.ID] = cloneProfile(profile)
	}

	merged := make([]Profile, 0, len(byID))
	for _, profile := range byID {
		merged = append(merged, cloneProfile(profile))
	}
	SortProfiles(merged)
	return merged, nil
}

func sameAgentLaunchIdentity(configured, discovered Profile) bool {
	return configured.Executable == discovered.Executable &&
		slices.Equal(configured.Args, discovered.Args) &&
		maps.Equal(configured.Env, discovered.Env) &&
		configured.CWDPolicy == discovered.CWDPolicy &&
		configured.FixedCWD == discovered.FixedCWD
}

func normalizeProfileSet(profiles []Profile, label string) ([]Profile, error) {
	result := make([]Profile, 0, len(profiles))
	seen := make(map[string]struct{}, len(profiles))
	for _, source := range profiles {
		if _, duplicate := seen[source.ID]; duplicate {
			return nil, fmt.Errorf("duplicate %s terminal profile ID %q", label, source.ID)
		}
		profile, err := ValidateProfile(source)
		if err != nil {
			return nil, fmt.Errorf("validate %s terminal profile %q: %w", label, source.ID, err)
		}
		seen[profile.ID] = struct{}{}
		result = append(result, profile)
	}
	return result, nil
}

// LoadProfileConfig reads one strictly decoded, bounded configuration file.
func LoadProfileConfig(path string) (ProfileConfig, error) {
	if path == "" {
		return ProfileConfig{}, errors.New("terminal profile config path is required")
	}
	file, err := os.Open(path)
	if err != nil {
		return ProfileConfig{}, fmt.Errorf("open terminal profile config: %w", err)
	}
	defer file.Close()

	contents, err := io.ReadAll(io.LimitReader(file, maxProfileConfigJSONBytes+1))
	if err != nil {
		return ProfileConfig{}, fmt.Errorf("read terminal profile config: %w", err)
	}
	if len(contents) == 0 {
		return ProfileConfig{}, errors.New("terminal profile config is empty")
	}
	if len(contents) > maxProfileConfigJSONBytes {
		return ProfileConfig{}, errors.New("terminal profile config is too large")
	}

	decoder := json.NewDecoder(bytes.NewReader(contents))
	decoder.DisallowUnknownFields()
	var config ProfileConfig
	if err := decoder.Decode(&config); err != nil {
		return ProfileConfig{}, fmt.Errorf("decode terminal profile config: %w", err)
	}
	if token, err := decoder.Token(); err != io.EOF || token != nil {
		return ProfileConfig{}, errors.New("terminal profile config has trailing data")
	}
	return ValidateProfileConfig(config)
}

// SaveProfileConfig atomically publishes private, normalized JSON at a
// caller-supplied path. Concurrent readers observe either the previous file or
// the complete replacement, never a partial write.
func SaveProfileConfig(path string, config ProfileConfig) error {
	if path == "" {
		return errors.New("terminal profile config path is required")
	}
	normalized, err := ValidateProfileConfig(config)
	if err != nil {
		return err
	}
	contents, err := json.MarshalIndent(normalized, "", "  ")
	if err != nil {
		return fmt.Errorf("encode terminal profile config: %w", err)
	}
	contents = append(contents, '\n')
	if len(contents) > maxProfileConfigJSONBytes {
		return errors.New("terminal profile config is too large")
	}

	directory := filepath.Dir(path)
	if err := os.MkdirAll(directory, 0o700); err != nil {
		return fmt.Errorf("create terminal profile config directory: %w", err)
	}
	temporary, err := os.CreateTemp(directory, ".terminal-profiles-*")
	if err != nil {
		return fmt.Errorf("create terminal profile config temporary file: %w", err)
	}
	temporaryPath := temporary.Name()
	removeTemporary := true
	defer func() {
		if removeTemporary {
			_ = os.Remove(temporaryPath)
		}
	}()

	chmodErr := temporary.Chmod(0o600)
	_, writeErr := temporary.Write(contents)
	syncErr := temporary.Sync()
	closeErr := temporary.Close()
	if err := errors.Join(chmodErr, writeErr, syncErr, closeErr); err != nil {
		return fmt.Errorf("write terminal profile config: %w", err)
	}
	if err := replaceProfileConfig(temporaryPath, path); err != nil {
		return fmt.Errorf("publish terminal profile config: %w", err)
	}
	removeTemporary = false
	if err := os.Chmod(path, 0o600); err != nil {
		return fmt.Errorf("secure terminal profile config: %w", err)
	}
	if err := syncProfileConfigDirectory(directory); err != nil {
		return fmt.Errorf("sync terminal profile config directory: %w", err)
	}
	return nil
}
