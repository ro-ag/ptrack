package capability

import (
	"errors"
	"fmt"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/ro-ag/ptrack/internal/model"
)

// GitAuthorization is a typed Git operation request. RemoteURL must be read
// from the repository immediately before authorization, not caller supplied.
type GitAuthorization struct {
	Operation  string
	RemoteName string
	RemoteURL  string
	Branch     string
	Refspec    string
	Force      bool
}

// ErrDenied is returned whenever a capability does not explicitly authorize
// an operation. Reasons are stable, sanitized policy messages.
type ErrDenied struct {
	Reason string
}

func (e ErrDenied) Error() string { return "capability denied: " + e.Reason }

// SSHOperation identifies one independently approved SSH behavior.
type SSHOperation string

const (
	SSHGit              SSHOperation = "git"
	SSHRemoteCommand    SSHOperation = "remote-command"
	SSHUpload           SSHOperation = "upload"
	SSHDownload         SSHOperation = "download"
	SSHInteractiveShell SSHOperation = "interactive-shell"
	SSHLocalForward     SSHOperation = "local-forward"
	SSHRemoteForward    SSHOperation = "remote-forward"
)

// Authorize verifies the common approval envelope and returns the normalized
// stored capability. Authorization is intentionally repeated immediately
// before every operation so disable, edit, and expiry take effect promptly.
func Authorize(capability model.Capability, agentProfile string, now time.Time) (model.Capability, error) {
	preview, err := Normalize(capability)
	if err != nil {
		return model.Capability{}, ErrDenied{Reason: "stored capability is invalid"}
	}
	normalized := preview.Capability
	if capability.ScopeDigest == "" || capability.ScopeDigest != preview.ScopeDigest {
		return model.Capability{}, ErrDenied{Reason: "approval scope is stale"}
	}
	if !normalized.Enabled {
		return model.Capability{}, ErrDenied{Reason: "capability is disabled"}
	}
	if normalized.AgentProfile != agentProfile {
		return model.Capability{}, ErrDenied{Reason: "agent profile does not match"}
	}
	if normalized.ApprovedAt.IsZero() || normalized.ExpiresAt.IsZero() {
		return model.Capability{}, ErrDenied{Reason: "capability has not been approved"}
	}
	if !normalized.ExpiresAt.After(now) {
		return model.Capability{}, ErrDenied{Reason: "capability approval has expired"}
	}
	maximumExpiry := normalized.ApprovedAt.Add(time.Duration(normalized.ApprovalDurationSeconds) * time.Second)
	if normalized.ExpiresAt.After(maximumExpiry) {
		return model.Capability{}, ErrDenied{Reason: "approval expiry exceeds its duration"}
	}
	return normalized, nil
}

// Approve enables a normalized capability only when the caller confirms the
// exact preview digest. It creates a fresh bounded approval window.
func Approve(capability model.Capability, expectedDigest string, now time.Time) (model.Capability, error) {
	preview, err := Normalize(capability)
	if err != nil {
		return model.Capability{}, err
	}
	if expectedDigest == "" || expectedDigest != preview.ScopeDigest {
		return model.Capability{}, errors.New("effective scope changed; preview again before enabling")
	}
	approved := preview.Capability
	approved.Enabled = true
	approved.ApprovedAt = now
	approved.ExpiresAt = now.Add(time.Duration(approved.ApprovalDurationSeconds) * time.Second)
	return approved, nil
}

// Disable revokes a capability without changing its normalized scope.
func Disable(capability model.Capability) model.Capability {
	capability.Enabled = false
	capability.ApprovedAt = time.Time{}
	capability.ExpiresAt = time.Time{}
	return capability
}

// AuthorizeSSH checks one SSH operation without allowing any grant to imply
// another. value is the exact approved command or forwarding endpoint when
// those operations require one.
func AuthorizeSSH(
	capability model.Capability,
	agentProfile string,
	now time.Time,
	operation SSHOperation,
	value string,
) (model.Capability, error) {
	normalized, err := Authorize(capability, agentProfile, now)
	if err != nil {
		return model.Capability{}, err
	}
	if normalized.Kind != model.CapabilitySSH || normalized.SSH == nil {
		return model.Capability{}, ErrDenied{Reason: "capability is not SSH"}
	}
	scope := normalized.SSH
	allowed := false
	switch operation {
	case SSHGit:
		allowed = scope.AllowGit
	case SSHRemoteCommand:
		allowed = contains(scope.RemoteCommands, value)
	case SSHUpload:
		allowed = scope.AllowUpload
	case SSHDownload:
		allowed = scope.AllowDownload
	case SSHInteractiveShell:
		allowed = scope.AllowInteractiveShell
	case SSHLocalForward:
		endpoint, normalizeErr := normalizeEndpoint(value)
		allowed = normalizeErr == nil && contains(scope.LocalForwardTargets, endpoint)
	case SSHRemoteForward:
		endpoint, normalizeErr := normalizeEndpoint(value)
		allowed = normalizeErr == nil && contains(scope.RemoteForwardTargets, endpoint)
	default:
		return model.Capability{}, ErrDenied{Reason: fmt.Sprintf("unknown SSH operation %q", operation)}
	}
	if !allowed {
		return model.Capability{}, ErrDenied{Reason: fmt.Sprintf("SSH %s is not approved", operation)}
	}
	return normalized, nil
}

// AuthorizeHTTP enforces the exact approved method, origin, segment-boundary
// path, and request-size limit. Query values are intentionally outside the
// persisted/audited scope but remain transient request data.
func AuthorizeHTTP(
	capability model.Capability,
	agentProfile string,
	now time.Time,
	method, rawURL string,
	requestBytes int64,
) (model.Capability, *url.URL, error) {
	normalized, err := Authorize(capability, agentProfile, now)
	if err != nil {
		return model.Capability{}, nil, err
	}
	if normalized.Kind != model.CapabilityHTTP || normalized.HTTP == nil {
		return model.Capability{}, nil, ErrDenied{Reason: "capability is not HTTP"}
	}
	method = strings.ToUpper(strings.TrimSpace(method))
	if !contains(normalized.HTTP.Methods, method) {
		return model.Capability{}, nil, ErrDenied{Reason: fmt.Sprintf("HTTP method %s is not approved", method)}
	}
	if requestBytes < 0 || requestBytes > normalized.Limits.MaxRequestBytes {
		return model.Capability{}, nil, ErrDenied{Reason: "HTTP request exceeds its byte limit"}
	}
	requestURL, err := normalizeHTTPURL(rawURL, true)
	if err != nil || requestURL.Fragment != "" {
		return model.Capability{}, nil, ErrDenied{Reason: "HTTP request URL is invalid"}
	}
	baseURL, err := url.Parse(normalized.HTTP.BaseURL)
	if err != nil || requestURL.Scheme != baseURL.Scheme || requestURL.Host != baseURL.Host {
		return model.Capability{}, nil, ErrDenied{Reason: "HTTP request origin is outside the approved scope"}
	}
	allowedPath := false
	for _, prefix := range normalized.HTTP.PathPrefixes {
		if pathWithin(prefix, requestURL.Path) {
			allowedPath = true
			break
		}
	}
	if !allowedPath {
		return model.Capability{}, nil, ErrDenied{Reason: "HTTP request path is outside the approved scope"}
	}
	return normalized, requestURL, nil
}

// AuthorizeGit checks a repository's freshly read remote configuration and a
// fixed typed operation. Empty branch/refspec allowlists authorize none.
func AuthorizeGit(
	capability model.Capability,
	agentProfile string,
	now time.Time,
	request GitAuthorization,
) (model.Capability, error) {
	normalized, err := Authorize(capability, agentProfile, now)
	if err != nil {
		return model.Capability{}, err
	}
	if normalized.Kind != model.CapabilityGit || normalized.Git == nil {
		return model.Capability{}, ErrDenied{Reason: "capability is not Git"}
	}
	scope := normalized.Git
	operation := strings.ToLower(strings.TrimSpace(request.Operation))
	if !contains(scope.Operations, operation) {
		return model.Capability{}, ErrDenied{Reason: fmt.Sprintf("Git %s is not approved", operation)}
	}
	remote, err := normalizeGitRemote(request.RemoteURL)
	if err != nil || request.RemoteName != scope.RemoteName || remote != scope.RemoteURL {
		return model.Capability{}, ErrDenied{Reason: "Git remote no longer matches the approved scope"}
	}
	if operation == "fetch" || operation == "pull" || operation == "push" {
		if !contains(scope.Branches, request.Branch) {
			return model.Capability{}, ErrDenied{Reason: "Git branch is not approved"}
		}
	}
	if request.Refspec != "" && !contains(scope.Refspecs, request.Refspec) {
		return model.Capability{}, ErrDenied{Reason: "Git refspec is not approved"}
	}
	if request.Force && !scope.AllowForcePush {
		return model.Capability{}, ErrDenied{Reason: "Git force push is not approved"}
	}
	if strings.HasPrefix(request.Refspec, ":") && !scope.AllowDeleteRefs {
		return model.Capability{}, ErrDenied{Reason: "Git ref deletion is not approved"}
	}
	if strings.Contains(request.Refspec, "refs/tags/") && !scope.AllowTags {
		return model.Capability{}, ErrDenied{Reason: "Git tag writes are not approved"}
	}
	return normalized, nil
}

// ResolveProjectPath canonicalizes an operation path and proves that it stays
// beneath both the canonical project root and one separately approved root.
// For a destination that does not exist, the nearest existing ancestor is
// resolved to prevent symlinked-parent escapes.
func ResolveProjectPath(projectRoot, requested string, approvedRoots []string, mustExist bool) (string, error) {
	canonicalProject, err := filepath.EvalSymlinks(projectRoot)
	if err != nil {
		return "", ErrDenied{Reason: "project root cannot be canonicalized"}
	}
	canonicalProject, err = filepath.Abs(canonicalProject)
	if err != nil {
		return "", ErrDenied{Reason: "project root cannot be canonicalized"}
	}
	relative, err := normalizeProjectPath(requested)
	if err != nil {
		return "", ErrDenied{Reason: "path is not project-relative"}
	}
	target, err := canonicalizePath(filepath.Join(canonicalProject, filepath.FromSlash(relative)), mustExist)
	if err != nil || !filesystemWithin(canonicalProject, target) {
		return "", ErrDenied{Reason: "path escapes the project"}
	}
	for _, approved := range approvedRoots {
		approvedRelative, normalizeErr := normalizeProjectPath(approved)
		if normalizeErr != nil {
			continue
		}
		root, resolveErr := canonicalizePath(filepath.Join(canonicalProject, filepath.FromSlash(approvedRelative)), false)
		if resolveErr == nil && filesystemWithin(root, target) {
			return target, nil
		}
	}
	return "", ErrDenied{Reason: "path is outside approved roots"}
}

func canonicalizePath(value string, mustExist bool) (string, error) {
	canonical, err := filepath.EvalSymlinks(value)
	if err == nil {
		return filepath.Abs(canonical)
	}
	if mustExist || !errors.Is(err, os.ErrNotExist) {
		return "", err
	}
	missing := []string{}
	cursor := value
	for {
		parent := filepath.Dir(cursor)
		if parent == cursor {
			return "", err
		}
		missing = append(missing, filepath.Base(cursor))
		cursor = parent
		canonicalParent, parentErr := filepath.EvalSymlinks(cursor)
		if parentErr != nil {
			if errors.Is(parentErr, os.ErrNotExist) {
				continue
			}
			return "", parentErr
		}
		for index := len(missing) - 1; index >= 0; index-- {
			canonicalParent = filepath.Join(canonicalParent, missing[index])
		}
		return filepath.Abs(canonicalParent)
	}
}

func filesystemWithin(parent, child string) bool {
	relative, err := filepath.Rel(parent, child)
	return err == nil && relative != ".." && !strings.HasPrefix(relative, ".."+string(filepath.Separator))
}

func contains(values []string, wanted string) bool {
	for _, value := range values {
		if value == wanted {
			return true
		}
	}
	return false
}
