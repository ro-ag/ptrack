package capability

import (
	"context"
	"fmt"
	"net/url"
	"strconv"
	"strings"
	"time"

	"github.com/ro-ag/ptrack/internal/model"
)

const globalAuditLimit = 5000

// AuditStore is the narrow persistence contract used by Recorder.
type AuditStore interface {
	AddCapabilityAuditBounded(model.CapabilityAudit, int, int) (model.CapabilityAudit, error)
}

// AuditEvent contains transient operation metadata. Target is sanitized by
// capability kind before persistence and ErrorClass is reduced to a fixed set.
type AuditEvent struct {
	Operation     string
	Target        string
	Success       bool
	ErrorClass    string
	Duration      time.Duration
	RequestBytes  int64
	ResponseBytes int64
	Redirects     int
}

// Recorder persists bounded metadata only. It deliberately has no fields for
// headers, bodies, terminal output, credentials, raw stderr, or arguments.
type Recorder struct {
	Store AuditStore
	Now   func() time.Time
}

// Record sanitizes and appends one event when the capability's audit policy is
// enabled. A nil Store is treated as an unavailable optional audit sink.
func (r Recorder) Record(_ context.Context, capability model.Capability, event AuditEvent) error {
	if !capability.Audit.Enabled || r.Store == nil {
		return nil
	}
	now := time.Now
	if r.Now != nil {
		now = r.Now
	}
	record := model.CapabilityAudit{
		CapabilityID:   capability.ID,
		AgentProfile:   sanitizeProfile(capability.AgentProfile),
		Kind:           capability.Kind,
		Operation:      sanitizeOperation(capability.Kind, event.Operation),
		Target:         sanitizeAuditTarget(capability.Kind, event.Target),
		Success:        event.Success,
		ErrorClass:     sanitizeErrorClass(event.Success, event.ErrorClass),
		DurationMillis: boundedDurationMillis(event.Duration),
		RequestBytes:   boundedCount(event.RequestBytes),
		ResponseBytes:  boundedCount(event.ResponseBytes),
		Redirects:      boundedRedirects(event.Redirects),
		CreatedAt:      now(),
	}
	_, err := r.Store.AddCapabilityAuditBounded(record, capability.Audit.RetainLast, globalAuditLimit)
	return err
}

var allowedErrorClasses = map[string]bool{
	"none": true, "denied": true, "dns": true, "routing": true, "vpn": true,
	"proxy": true, "tls": true, "host-key": true, "authentication": true,
	"sandbox": true, "remote-policy": true, "timeout": true, "transport": true,
	"request-limit": true, "response-limit": true, "output-limit": true,
	"cancelled": true, "internal": true,
}

func sanitizeErrorClass(success bool, value string) string {
	if success {
		return "none"
	}
	value = strings.ToLower(strings.TrimSpace(value))
	if allowedErrorClasses[value] {
		return value
	}
	return "internal"
}

func sanitizeOperation(kind model.CapabilityKind, value string) string {
	value = strings.ToLower(strings.TrimSpace(value))
	allowed := false
	switch kind {
	case model.CapabilityHTTP:
		allowed = allowedHTTPMethods[strings.ToUpper(value)] || value == "test"
	case model.CapabilityGit:
		allowed = contains([]string{"status", "fetch", "pull", "push", "ls-remote", "test"}, value)
	case model.CapabilitySSH:
		allowed = contains([]string{
			string(SSHGit), string(SSHRemoteCommand), string(SSHUpload), string(SSHDownload),
			string(SSHInteractiveShell), string(SSHLocalForward), string(SSHRemoteForward), "test",
		}, value)
	}
	if !allowed {
		return "unknown"
	}
	return value
}

func sanitizeAuditTarget(kind model.CapabilityKind, raw string) string {
	switch kind {
	case model.CapabilityHTTP:
		u, err := url.Parse(raw)
		if err != nil || u.Scheme == "" || u.Host == "" {
			return "invalid-http-target"
		}
		// Paths are caller-controlled and commonly carry opaque IDs or
		// credentials. Persist only the approved network origin.
		return truncate(u.Scheme+"://"+u.Host, 256)
	case model.CapabilityGit:
		name, err := normalizeToken(raw, "Git remote")
		if err != nil {
			return "invalid-git-target"
		}
		return truncate("remote:"+name, 160)
	case model.CapabilitySSH:
		host, port, err := netSplitAuditTarget(raw)
		if err != nil {
			return "invalid-ssh-target"
		}
		return truncate(host+":"+strconv.Itoa(port), 256)
	default:
		return "unknown-target"
	}
}

func netSplitAuditTarget(raw string) (string, int, error) {
	value := strings.TrimSpace(raw)
	colon := strings.LastIndex(value, ":")
	if colon <= 0 {
		return "", 0, fmt.Errorf("missing port")
	}
	host, err := normalizeHost(strings.Trim(value[:colon], "[]"))
	if err != nil {
		return "", 0, err
	}
	port, err := strconv.Atoi(value[colon+1:])
	if err != nil || port < 1 || port > 65535 {
		return "", 0, fmt.Errorf("invalid port")
	}
	return host, port, nil
}

func sanitizeProfile(value string) string {
	profile, err := normalizeProfile(value)
	if err != nil {
		return "unknown-profile"
	}
	return profile
}

func boundedDurationMillis(value time.Duration) int64 {
	if value < 0 {
		return 0
	}
	maximum := 24 * time.Hour
	if value > maximum {
		value = maximum
	}
	return value.Milliseconds()
}

func boundedCount(value int64) int64 {
	if value < 0 {
		return 0
	}
	const maximum = int64(1 << 40)
	if value > maximum {
		return maximum
	}
	return value
}

func boundedRedirects(value int) int {
	if value < 0 {
		return 0
	}
	if value > maxRedirects {
		return maxRedirects
	}
	return value
}

func truncate(value string, maximum int) string {
	if len(value) <= maximum {
		return value
	}
	return value[:maximum]
}
