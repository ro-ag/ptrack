// Package capability implements normalization, authorization, and execution
// for p-track's explicit project host capabilities.
package capability

import (
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"net/url"
	"path"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"unicode"

	"github.com/ro-ag/ptrack/internal/model"
)

const (
	defaultApprovalSeconds = int64(3600)
	maxApprovalSeconds     = int64(30 * 24 * 3600)
	defaultTimeoutSeconds  = 30
	maxTimeoutSeconds      = 300
	defaultRequestBytes    = int64(1 << 20)
	defaultResponseBytes   = int64(4 << 20)
	defaultOutputBytes     = int64(1 << 20)
	maxTransferBytes       = int64(32 << 20)
	maxRedirects           = 10
	defaultConcurrent      = 1
	maxConcurrent          = 8
	defaultAuditRecords    = 100
	maxAuditRecords        = 1000
)

// Preview is the canonical capability contract shown before approval.
type Preview struct {
	Capability     model.Capability `json:"capability"`
	EffectiveScope string           `json:"effective_scope"`
	ScopeDigest    string           `json:"scope_digest"`
}

// Normalize validates and canonicalizes a draft capability. It never enables
// a grant or extends approval; callers must explicitly approve the returned
// digest after presenting EffectiveScope.
func Normalize(input model.Capability) (Preview, error) {
	capability := input
	capability.Name = strings.TrimSpace(capability.Name)
	if capability.Name == "" || len(capability.Name) > 128 || hasControl(capability.Name) {
		return Preview{}, errors.New("capability name must be 1-128 printable characters")
	}
	if capability.ModelVersion == 0 {
		capability.ModelVersion = model.CapabilityModelVersion
	}
	if capability.ModelVersion != model.CapabilityModelVersion {
		return Preview{}, fmt.Errorf("unsupported capability model version %d", capability.ModelVersion)
	}
	profile, err := normalizeProfile(capability.AgentProfile)
	if err != nil {
		return Preview{}, err
	}
	capability.AgentProfile = profile
	if capability.ApprovalDurationSeconds == 0 {
		capability.ApprovalDurationSeconds = defaultApprovalSeconds
	}
	if capability.ApprovalDurationSeconds < 60 || capability.ApprovalDurationSeconds > maxApprovalSeconds {
		return Preview{}, fmt.Errorf("approval duration must be between 60 and %d seconds", maxApprovalSeconds)
	}
	if err := normalizeLimits(&capability.Limits); err != nil {
		return Preview{}, err
	}
	if capability.Audit.RetainLast == 0 {
		capability.Audit.RetainLast = defaultAuditRecords
	}
	if capability.Audit.RetainLast < 0 || capability.Audit.RetainLast > maxAuditRecords {
		return Preview{}, fmt.Errorf("audit retention must be between 0 and %d records", maxAuditRecords)
	}

	var effective string
	switch capability.Kind {
	case model.CapabilityHTTP:
		if capability.HTTP == nil || capability.Git != nil || capability.SSH != nil {
			return Preview{}, errors.New("HTTP capability must contain only an HTTP scope")
		}
		effective, err = normalizeHTTP(capability.HTTP)
	case model.CapabilityGit:
		if capability.Git == nil || capability.HTTP != nil || capability.SSH != nil {
			return Preview{}, errors.New("Git capability must contain only a Git scope")
		}
		effective, err = normalizeGit(capability.Git)
	case model.CapabilitySSH:
		if capability.SSH == nil || capability.HTTP != nil || capability.Git != nil {
			return Preview{}, errors.New("SSH capability must contain only an SSH scope")
		}
		effective, err = normalizeSSH(capability.SSH)
	default:
		return Preview{}, fmt.Errorf("unsupported capability kind %q", capability.Kind)
	}
	if err != nil {
		return Preview{}, err
	}
	digest, err := scopeDigest(capability)
	if err != nil {
		return Preview{}, err
	}
	capability.ScopeDigest = digest
	effective = effectiveApprovalScope(capability, effective)
	return Preview{Capability: capability, EffectiveScope: effective, ScopeDigest: digest}, nil
}

func normalizeProfile(raw string) (string, error) {
	value := strings.TrimSpace(raw)
	if len(value) == 0 || len(value) > 64 || value[0] == '-' || hasControl(value) {
		return "", errors.New("agent profile must be an explicit 1-64 character profile ID")
	}
	for _, r := range value {
		if !unicode.IsLetter(r) && !unicode.IsDigit(r) && !strings.ContainsRune("._-", r) {
			return "", fmt.Errorf("invalid agent profile %q", raw)
		}
	}
	return value, nil
}

func normalizeLimits(limits *model.CapabilityLimits) error {
	if limits.TimeoutSeconds == 0 {
		limits.TimeoutSeconds = defaultTimeoutSeconds
	}
	if limits.MaxRequestBytes == 0 {
		limits.MaxRequestBytes = defaultRequestBytes
	}
	if limits.MaxResponseBytes == 0 {
		limits.MaxResponseBytes = defaultResponseBytes
	}
	if limits.MaxOutputBytes == 0 {
		limits.MaxOutputBytes = defaultOutputBytes
	}
	if limits.MaxConcurrent == 0 {
		limits.MaxConcurrent = defaultConcurrent
	}
	if limits.TimeoutSeconds < 1 || limits.TimeoutSeconds > maxTimeoutSeconds {
		return fmt.Errorf("timeout must be between 1 and %d seconds", maxTimeoutSeconds)
	}
	for name, value := range map[string]int64{
		"request": limits.MaxRequestBytes, "response": limits.MaxResponseBytes, "output": limits.MaxOutputBytes,
	} {
		if value < 1 || value > maxTransferBytes {
			return fmt.Errorf("maximum %s bytes must be between 1 and %d", name, maxTransferBytes)
		}
	}
	if limits.MaxRedirects < 0 || limits.MaxRedirects > maxRedirects {
		return fmt.Errorf("maximum redirects must be between 0 and %d", maxRedirects)
	}
	if limits.MaxConcurrent < 1 || limits.MaxConcurrent > maxConcurrent {
		return fmt.Errorf("maximum concurrent operations must be between 1 and %d", maxConcurrent)
	}
	return nil
}

var allowedHTTPMethods = map[string]bool{
	"GET": true, "HEAD": true, "POST": true, "PUT": true, "PATCH": true, "DELETE": true, "OPTIONS": true,
}

func normalizeHTTP(scope *model.HTTPScope) (string, error) {
	base, err := normalizeHTTPURL(scope.BaseURL, false)
	if err != nil {
		return "", fmt.Errorf("HTTP base URL: %w", err)
	}
	if base.RawQuery != "" || base.Fragment != "" {
		return "", errors.New("HTTP base URL cannot contain a query or fragment")
	}
	base.Path = normalizeWebPath(base.Path)
	base.RawPath = ""
	scope.BaseURL = base.String()
	methods := make([]string, 0, len(scope.Methods))
	for _, raw := range scope.Methods {
		method := strings.ToUpper(strings.TrimSpace(raw))
		if !allowedHTTPMethods[method] {
			return "", fmt.Errorf("HTTP method %q is not supported", raw)
		}
		methods = append(methods, method)
	}
	scope.Methods = uniqueSorted(methods)
	if len(scope.Methods) == 0 {
		return "", errors.New("HTTP scope must approve at least one method")
	}
	if len(scope.PathPrefixes) == 0 {
		scope.PathPrefixes = []string{base.Path}
	}
	paths := make([]string, 0, len(scope.PathPrefixes))
	for _, raw := range scope.PathPrefixes {
		prefix, err := normalizeScopePath(raw)
		if err != nil {
			return "", err
		}
		if !pathWithin(base.Path, prefix) {
			return "", fmt.Errorf("HTTP path %q is outside base path %q", prefix, base.Path)
		}
		paths = append(paths, prefix)
	}
	scope.PathPrefixes = uniqueSorted(paths)
	return fmt.Sprintf("%s %s paths=%s", strings.Join(scope.Methods, ","), scope.BaseURL, strings.Join(scope.PathPrefixes, ",")), nil
}

func normalizeHTTPURL(raw string, allowQuery bool) (*url.URL, error) {
	value := strings.TrimSpace(raw)
	if hasControl(value) {
		return nil, errors.New("URL contains control characters")
	}
	u, err := url.ParseRequestURI(value)
	if err != nil || !u.IsAbs() || u.Opaque != "" {
		return nil, errors.New("URL must be an absolute hierarchical URL")
	}
	u.Scheme = strings.ToLower(u.Scheme)
	if u.Scheme != "http" && u.Scheme != "https" {
		return nil, errors.New("URL scheme must be http or https")
	}
	if u.User != nil {
		return nil, errors.New("embedded URL credentials are not allowed")
	}
	if !allowQuery && u.RawQuery != "" {
		return nil, errors.New("URL query is not allowed in an approved scope")
	}
	host, port, err := normalizeURLHost(u.Hostname(), u.Port(), u.Scheme)
	if err != nil {
		return nil, err
	}
	u.Host = host
	if port != "" {
		u.Host = net.JoinHostPort(host, port)
	}
	if strings.Contains(strings.ToLower(u.EscapedPath()), "%2f") || strings.Contains(strings.ToLower(u.EscapedPath()), "%5c") || strings.Contains(strings.ToLower(u.EscapedPath()), "%2e") {
		return nil, errors.New("encoded slash, backslash, or dot path segments are not allowed")
	}
	decoded, err := url.PathUnescape(u.EscapedPath())
	if err != nil || strings.Contains(decoded, "\\") || strings.Contains(decoded, "%") || hasControl(decoded) {
		return nil, errors.New("URL path is ambiguous")
	}
	u.Path = normalizeWebPath(decoded)
	u.RawPath = ""
	return u, nil
}

func normalizeURLHost(rawHost, rawPort, scheme string) (string, string, error) {
	host, err := normalizeHost(rawHost)
	if err != nil {
		return "", "", err
	}
	port := rawPort
	if port != "" {
		n, err := strconv.Atoi(port)
		if err != nil || n < 1 || n > 65535 {
			return "", "", errors.New("port must be between 1 and 65535")
		}
		if scheme == "http" && port == "80" || scheme == "https" && port == "443" || scheme == "ssh" && port == "22" {
			port = ""
		}
	}
	return host, port, nil
}

func normalizeScopePath(raw string) (string, error) {
	if !strings.HasPrefix(raw, "/") || strings.ContainsAny(raw, "?#") || hasControl(raw) {
		return "", fmt.Errorf("HTTP path prefix %q must be an absolute path without query or fragment", raw)
	}
	lower := strings.ToLower(raw)
	if strings.Contains(lower, "%2f") || strings.Contains(lower, "%5c") || strings.Contains(lower, "%2e") {
		return "", fmt.Errorf("HTTP path prefix %q contains ambiguous encoding", raw)
	}
	decoded, err := url.PathUnescape(raw)
	if err != nil || strings.Contains(decoded, "\\") || strings.Contains(decoded, "%") {
		return "", fmt.Errorf("HTTP path prefix %q is ambiguous", raw)
	}
	return normalizeWebPath(decoded), nil
}

func normalizeWebPath(value string) string {
	clean := path.Clean("/" + strings.TrimPrefix(value, "/"))
	if clean == "." {
		return "/"
	}
	return clean
}

func pathWithin(parent, child string) bool {
	parent = normalizeWebPath(parent)
	child = normalizeWebPath(child)
	return parent == "/" || child == parent || strings.HasPrefix(child, strings.TrimSuffix(parent, "/")+"/")
}

func normalizeGit(scope *model.GitScope) (string, error) {
	remoteName, err := normalizeToken(scope.RemoteName, "Git remote name")
	if err != nil {
		return "", err
	}
	scope.RemoteName = remoteName
	scope.RemoteURL, err = normalizeGitRemote(scope.RemoteURL)
	if err != nil {
		return "", err
	}
	allowed := map[string]bool{"status": true, "fetch": true, "pull": true, "push": true, "ls-remote": true}
	operations := make([]string, 0, len(scope.Operations))
	for _, raw := range scope.Operations {
		op := strings.ToLower(strings.TrimSpace(raw))
		if !allowed[op] {
			return "", fmt.Errorf("Git operation %q is not supported", raw)
		}
		operations = append(operations, op)
	}
	scope.Operations = uniqueSorted(operations)
	if len(scope.Operations) == 0 {
		return "", errors.New("Git scope must approve at least one operation")
	}
	for _, branch := range scope.Branches {
		if err := validateGitBranch(branch); err != nil {
			return "", fmt.Errorf("Git branch %q: %w", branch, err)
		}
	}
	scope.Branches = uniqueSorted(scope.Branches)
	refspecs := make([]string, 0, len(scope.Refspecs))
	for _, refspec := range scope.Refspecs {
		normalized, err := normalizeRefspec(refspec, *scope)
		if err != nil {
			return "", err
		}
		refspecs = append(refspecs, normalized)
	}
	scope.Refspecs = uniqueSorted(refspecs)
	return fmt.Sprintf(
		"remote %s=%s operations=%s branches=%s refspecs=%s allow_tags=%t allow_force_with_lease=%t allow_delete_refs=%t",
		scope.RemoteName,
		scope.RemoteURL,
		scopeList(scope.Operations),
		scopeList(scope.Branches),
		scopeList(scope.Refspecs),
		scope.AllowTags,
		scope.AllowForcePush,
		scope.AllowDeleteRefs,
	), nil
}

func normalizeGitRemote(raw string) (string, error) {
	value := strings.TrimSpace(raw)
	if value == "" || value[0] == '-' || hasControl(value) {
		return "", errors.New("Git remote URL is invalid")
	}
	if strings.Contains(value, "://") {
		u, err := url.Parse(value)
		if err != nil || u.Opaque != "" || u.Hostname() == "" || u.RawQuery != "" || u.Fragment != "" {
			return "", errors.New("Git remote must be an absolute URL without query or fragment")
		}
		u.Scheme = strings.ToLower(u.Scheme)
		if u.Scheme != "https" && u.Scheme != "ssh" {
			return "", errors.New("Git remote scheme must be https or ssh")
		}
		if u.User != nil {
			if u.Scheme != "ssh" {
				return "", errors.New("embedded Git credentials are not allowed")
			}
			if _, passwordSet := u.User.Password(); passwordSet {
				return "", errors.New("embedded Git credentials are not allowed")
			}
		}
		host, port, err := normalizeURLHost(u.Hostname(), u.Port(), u.Scheme)
		if err != nil {
			return "", err
		}
		authority := host
		if port != "" {
			authority = net.JoinHostPort(host, port)
		}
		if u.User != nil {
			user, err := normalizeToken(u.User.Username(), "SSH user")
			if err != nil {
				return "", err
			}
			authority = user + "@" + authority
		}
		u.Host = authority
		cleanPath, err := normalizeGitPath(u.Path)
		if err != nil {
			return "", err
		}
		u.Path = cleanPath
		u.RawPath = ""
		return u.String(), nil
	}

	colon := strings.Index(value, ":")
	if colon <= 0 || colon == len(value)-1 {
		return "", errors.New("Git remote must use https, ssh, or SCP-style SSH syntax")
	}
	identity, remotePath := value[:colon], value[colon+1:]
	user := ""
	host := identity
	if at := strings.LastIndex(identity, "@"); at >= 0 {
		user = identity[:at]
		host = identity[at+1:]
		if _, err := normalizeToken(user, "SSH user"); err != nil {
			return "", err
		}
	}
	host, err := normalizeHost(host)
	if err != nil {
		return "", err
	}
	remotePath, err = normalizeGitPath(remotePath)
	if err != nil {
		return "", err
	}
	if user != "" {
		host = user + "@" + host
	}
	return host + ":" + strings.TrimPrefix(remotePath, "/"), nil
}

func normalizeGitPath(raw string) (string, error) {
	if raw == "" || hasControl(raw) || strings.Contains(raw, "\\") {
		return "", errors.New("Git remote path is invalid")
	}
	for _, segment := range strings.Split(raw, "/") {
		if segment == ".." || segment == "." {
			return "", errors.New("Git remote path cannot contain dot segments")
		}
	}
	return path.Clean(raw), nil
}

func validateGitRef(ref string) error {
	if ref == "" || ref[0] == '-' || strings.HasSuffix(ref, ".") || strings.HasSuffix(ref, "/") || strings.Contains(ref, "..") || strings.Contains(ref, "@{") || strings.ContainsAny(ref, " ~^:?*[\\") || hasControl(ref) {
		return errors.New("invalid ref name")
	}
	for _, part := range strings.Split(ref, "/") {
		if part == "" || part == "." || part == ".." || strings.HasPrefix(part, ".") || strings.HasSuffix(part, ".lock") {
			return errors.New("invalid ref component")
		}
	}
	return nil
}

func validateGitBranch(branch string) error {
	upper := strings.ToUpper(branch)
	if strings.HasPrefix(branch, "+") || strings.HasPrefix(branch, "refs/") || strings.Contains(branch, ":") ||
		branch == "@" || upper == "HEAD" || strings.HasSuffix(upper, "_HEAD") || upper == "AUTO_MERGE" {
		return errors.New("branch must be an unqualified head name")
	}
	return validateGitRef(branch)
}

func normalizeRefspec(refspec string, scope model.GitScope) (string, error) {
	if refspec == "" || refspec[0] == '-' || strings.Contains(refspec, "*") || hasControl(refspec) {
		return "", fmt.Errorf("Git refspec %q is invalid", refspec)
	}
	value := refspec
	if strings.HasPrefix(value, "+") {
		return "", fmt.Errorf("Git refspec %q uses unconditional force; use the separate force-with-lease request", refspec)
	}
	parts := strings.Split(value, ":")
	if len(parts) > 2 {
		return "", fmt.Errorf("Git refspec %q is invalid", refspec)
	}
	if len(parts) == 2 && parts[0] == "" && !scope.AllowDeleteRefs {
		return "", fmt.Errorf("Git refspec %q requires ref-deletion approval", refspec)
	}
	for index, ref := range parts {
		if ref == "" {
			continue
		}
		switch {
		case strings.HasPrefix(ref, "refs/heads/"):
			branch := strings.TrimPrefix(ref, "refs/heads/")
			if err := validateGitBranch(branch); err != nil {
				return "", fmt.Errorf("Git refspec %q is invalid", refspec)
			}
			parts[index] = "refs/heads/" + branch
		case strings.HasPrefix(ref, "refs/tags/"):
			tag := strings.TrimPrefix(ref, "refs/tags/")
			if !scope.AllowTags {
				return "", fmt.Errorf("Git refspec %q requires tag approval", refspec)
			}
			if err := validateGitRef(tag); err != nil {
				return "", fmt.Errorf("Git refspec %q is invalid", refspec)
			}
			parts[index] = "refs/tags/" + tag
		case strings.HasPrefix(ref, "refs/"):
			return "", fmt.Errorf("Git refspec %q uses an unsupported namespace", refspec)
		default:
			if err := validateGitBranch(ref); err != nil {
				return "", fmt.Errorf("Git refspec %q is invalid", refspec)
			}
			parts[index] = "refs/heads/" + ref
		}
	}
	return strings.Join(parts, ":"), nil
}

func normalizeSSH(scope *model.SSHScope) (string, error) {
	var err error
	if scope.AllowInteractiveShell {
		return "", errors.New("interactive SSH shells are unavailable through the capability broker transport")
	}
	if scope.Alias != "" {
		scope.Alias, err = normalizeToken(scope.Alias, "SSH alias")
		if err != nil {
			return "", err
		}
	}
	scope.Host, err = normalizeHost(scope.Host)
	if err != nil {
		return "", err
	}
	if scope.Port == 0 {
		scope.Port = 22
	}
	if scope.User == "" {
		return "", errors.New("SSH user is required")
	}
	scope.User, err = normalizeToken(scope.User, "SSH user")
	if err != nil {
		return "", err
	}
	if err := validateHostKey(scope.HostKey); err != nil {
		return "", err
	}
	for _, command := range scope.RemoteCommands {
		if strings.TrimSpace(command) != command || command == "" || strings.ContainsAny(command, "\r\n\x00") {
			return "", errors.New("SSH remote commands must be exact non-empty single-line strings")
		}
	}
	scope.RemoteCommands = uniqueSorted(scope.RemoteCommands)
	if scope.AllowUpload && (len(scope.UploadRoots) == 0 || len(scope.UploadRemoteRoots) == 0) || !scope.AllowUpload && (len(scope.UploadRoots) > 0 || len(scope.UploadRemoteRoots) > 0) {
		return "", errors.New("SSH local and remote upload roots must be configured with upload approval")
	}
	if scope.AllowDownload && (len(scope.DownloadRoots) == 0 || len(scope.DownloadRemoteRoots) == 0) || !scope.AllowDownload && (len(scope.DownloadRoots) > 0 || len(scope.DownloadRemoteRoots) > 0) {
		return "", errors.New("SSH local and remote download roots must be configured with download approval")
	}
	for index, root := range scope.UploadRoots {
		scope.UploadRoots[index], err = normalizeProjectPath(root)
		if err != nil {
			return "", fmt.Errorf("SSH upload root: %w", err)
		}
	}
	for index, root := range scope.DownloadRoots {
		scope.DownloadRoots[index], err = normalizeProjectPath(root)
		if err != nil {
			return "", fmt.Errorf("SSH download root: %w", err)
		}
	}
	for index, root := range scope.UploadRemoteRoots {
		scope.UploadRemoteRoots[index], err = normalizeRemotePath(root)
		if err != nil {
			return "", fmt.Errorf("SSH remote upload root: %w", err)
		}
	}
	for index, root := range scope.DownloadRemoteRoots {
		scope.DownloadRemoteRoots[index], err = normalizeRemotePath(root)
		if err != nil {
			return "", fmt.Errorf("SSH remote download root: %w", err)
		}
	}
	for index, target := range scope.LocalForwardTargets {
		scope.LocalForwardTargets[index], err = normalizeEndpoint(target)
		if err != nil {
			return "", fmt.Errorf("SSH local forwarding target: %w", err)
		}
	}
	for index, target := range scope.RemoteForwardTargets {
		scope.RemoteForwardTargets[index], err = normalizeEndpoint(target)
		if err != nil {
			return "", fmt.Errorf("SSH remote forwarding target: %w", err)
		}
	}
	scope.UploadRoots = uniqueSorted(scope.UploadRoots)
	scope.DownloadRoots = uniqueSorted(scope.DownloadRoots)
	scope.UploadRemoteRoots = uniqueSorted(scope.UploadRemoteRoots)
	scope.DownloadRemoteRoots = uniqueSorted(scope.DownloadRemoteRoots)
	scope.LocalForwardTargets = uniqueSorted(scope.LocalForwardTargets)
	scope.RemoteForwardTargets = uniqueSorted(scope.RemoteForwardTargets)
	grants := []string{}
	if scope.AllowGit {
		grants = append(grants, "git")
	}
	if len(scope.RemoteCommands) > 0 {
		grants = append(grants, "commands")
	}
	if scope.AllowUpload {
		grants = append(grants, "upload")
	}
	if scope.AllowDownload {
		grants = append(grants, "download")
	}
	if len(scope.LocalForwardTargets) > 0 {
		grants = append(grants, "local-forward")
	}
	if len(scope.RemoteForwardTargets) > 0 {
		grants = append(grants, "remote-forward")
	}
	if len(grants) == 0 {
		return "", errors.New("SSH scope must approve at least one operation")
	}
	return fmt.Sprintf(
		"alias=%q %s@%s:%d host-key=%s grants=%s commands=%s upload_local_roots=%s upload_remote_roots=%s download_local_roots=%s download_remote_roots=%s local_forward_targets=%s remote_forward_targets=%s",
		scope.Alias,
		scope.User,
		scope.Host,
		scope.Port,
		hostKeyFingerprint(scope.HostKey),
		scopeList(grants),
		scopeList(scope.RemoteCommands),
		scopeList(scope.UploadRoots),
		scopeList(scope.UploadRemoteRoots),
		scopeList(scope.DownloadRoots),
		scopeList(scope.DownloadRemoteRoots),
		scopeList(scope.LocalForwardTargets),
		scopeList(scope.RemoteForwardTargets),
	), nil
}

func normalizeRemotePath(raw string) (string, error) {
	if !strings.HasPrefix(raw, "/") || hasControl(raw) || strings.Contains(raw, "\\") {
		return "", errors.New("remote path must be an absolute POSIX path")
	}
	for _, r := range raw {
		if !unicode.IsLetter(r) && !unicode.IsDigit(r) && !strings.ContainsRune("/._-", r) {
			return "", errors.New("remote path contains shell-significant characters")
		}
	}
	clean := path.Clean(raw)
	if clean == "/" {
		return "", errors.New("remote root cannot be the entire host filesystem")
	}
	return clean, nil
}

func remotePathWithin(root, candidate string) bool {
	root, errRoot := normalizeRemotePath(root)
	candidate, errCandidate := normalizeRemotePath(candidate)
	return errRoot == nil && errCandidate == nil && pathWithin(root, candidate)
}

func normalizeProjectPath(raw string) (string, error) {
	if raw == "" || filepath.IsAbs(raw) || hasControl(raw) {
		return "", errors.New("path must be project-relative")
	}
	clean := filepath.Clean(raw)
	if clean == ".." || strings.HasPrefix(clean, ".."+string(filepath.Separator)) {
		return "", errors.New("path escapes the project")
	}
	return filepath.ToSlash(clean), nil
}

func normalizeEndpoint(raw string) (string, error) {
	host, port, err := net.SplitHostPort(raw)
	if err != nil {
		return "", errors.New("endpoint must be host:port")
	}
	host, err = normalizeHost(host)
	if err != nil {
		return "", err
	}
	n, err := strconv.Atoi(port)
	if err != nil || n < 1 || n > 65535 {
		return "", errors.New("endpoint port must be between 1 and 65535")
	}
	return net.JoinHostPort(host, port), nil
}

func validateHostKey(raw string) error {
	fields := strings.Fields(raw)
	if len(fields) != 2 || !strings.HasPrefix(fields[0], "ssh-") && !strings.HasPrefix(fields[0], "ecdsa-") {
		return errors.New("SSH host key must contain exactly a key type and base64 public key")
	}
	if _, err := base64.StdEncoding.DecodeString(fields[1]); err != nil {
		return errors.New("SSH host key is not valid base64")
	}
	return nil
}

func hostKeyFingerprint(raw string) string {
	fields := strings.Fields(raw)
	if len(fields) != 2 {
		return "invalid"
	}
	decoded, err := base64.StdEncoding.DecodeString(fields[1])
	if err != nil {
		return "invalid"
	}
	sum := sha256.Sum256(decoded)
	return "SHA256:" + base64.RawStdEncoding.EncodeToString(sum[:])
}

func normalizeHost(raw string) (string, error) {
	host := strings.TrimSuffix(strings.ToLower(strings.TrimSpace(strings.Trim(raw, "[]"))), ".")
	if host == "" || host[0] == '-' || hasControl(host) || strings.ContainsAny(host, "/\\@") {
		return "", fmt.Errorf("invalid host %q", raw)
	}
	if ip := net.ParseIP(host); ip != nil {
		return ip.String(), nil
	}
	if len(host) > 253 {
		return "", errors.New("host name is too long")
	}
	for _, label := range strings.Split(host, ".") {
		if label == "" || len(label) > 63 || label[0] == '-' || label[len(label)-1] == '-' {
			return "", fmt.Errorf("invalid host %q", raw)
		}
		for _, r := range label {
			if !unicode.IsLetter(r) && !unicode.IsDigit(r) && r != '-' {
				return "", fmt.Errorf("invalid host %q", raw)
			}
		}
	}
	return host, nil
}

func normalizeToken(raw, label string) (string, error) {
	value := strings.TrimSpace(raw)
	if value == "" || value[0] == '-' || len(value) > 128 || hasControl(value) {
		return "", fmt.Errorf("%s is invalid", label)
	}
	for _, r := range value {
		if !unicode.IsLetter(r) && !unicode.IsDigit(r) && !strings.ContainsRune("._-/", r) {
			return "", fmt.Errorf("%s %q is invalid", label, raw)
		}
	}
	return value, nil
}

func uniqueSorted(values []string) []string {
	seen := make(map[string]struct{}, len(values))
	result := make([]string, 0, len(values))
	for _, value := range values {
		if _, exists := seen[value]; exists {
			continue
		}
		seen[value] = struct{}{}
		result = append(result, value)
	}
	sort.Strings(result)
	return result
}

func hasControl(value string) bool {
	return strings.IndexFunc(value, unicode.IsControl) >= 0
}

func effectiveApprovalScope(capability model.Capability, kindScope string) string {
	return fmt.Sprintf(
		"model_version=%d kind=%s profile=%s approval_duration_seconds=%d\nlimits timeout_seconds=%d max_request_bytes=%d max_response_bytes=%d max_output_bytes=%d max_redirects=%d max_concurrent=%d\naudit enabled=%t retain_last=%d\nscope %s",
		capability.ModelVersion,
		capability.Kind,
		capability.AgentProfile,
		capability.ApprovalDurationSeconds,
		capability.Limits.TimeoutSeconds,
		capability.Limits.MaxRequestBytes,
		capability.Limits.MaxResponseBytes,
		capability.Limits.MaxOutputBytes,
		capability.Limits.MaxRedirects,
		capability.Limits.MaxConcurrent,
		capability.Audit.Enabled,
		capability.Audit.RetainLast,
		kindScope,
	)
}

func scopeList(values []string) string {
	encoded, _ := json.Marshal(values)
	return string(encoded)
}

func scopeDigest(capability model.Capability) (string, error) {
	envelope := struct {
		ModelVersion            uint
		Kind                    model.CapabilityKind
		AgentProfile            string
		ApprovalDurationSeconds int64
		Limits                  model.CapabilityLimits
		Audit                   model.CapabilityAuditPolicy
		HTTP                    *model.HTTPScope
		Git                     *model.GitScope
		SSH                     *model.SSHScope
	}{
		capability.ModelVersion, capability.Kind, capability.AgentProfile,
		capability.ApprovalDurationSeconds, capability.Limits, capability.Audit,
		capability.HTTP, capability.Git, capability.SSH,
	}
	encoded, err := json.Marshal(envelope)
	if err != nil {
		return "", err
	}
	sum := sha256.Sum256(encoded)
	return hex.EncodeToString(sum[:]), nil
}
