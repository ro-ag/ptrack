package capability

import (
	"bytes"
	"context"
	"crypto/x509"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"os"
	"strings"
	"syscall"
	"time"

	"github.com/ro-ag/ptrack/internal/model"
)

const maxTransientHeaderBytes = 64 << 10

// HTTPRequest is a typed transient broker request. Headers and Body are never
// sent to the audit recorder.
type HTTPRequest struct {
	Method  string              `json:"method"`
	URL     string              `json:"url"`
	Headers map[string][]string `json:"headers,omitempty"`
	Body    []byte              `json:"body,omitempty"`
}

// HTTPResponse is bounded by the capability before being returned.
type HTTPResponse struct {
	StatusCode   int                 `json:"status_code"`
	Status       string              `json:"status"`
	Headers      map[string][]string `json:"headers"`
	Body         []byte              `json:"body"`
	EffectiveURL string              `json:"effective_url"`
	Redirects    int                 `json:"redirects"`
	Diagnostics  HTTPDiagnostics     `json:"diagnostics"`
}

// HTTPDiagnostics reports the host networking policy in use without exposing
// proxy credentials or replacing the system certificate store.
type HTTPDiagnostics struct {
	Proxy   string `json:"proxy"`
	CAStore string `json:"ca_store"`
}

// HTTPExecutor performs capability-authorized HTTP work.
type HTTPExecutor struct {
	Transport http.RoundTripper
	Proxy     func(*http.Request) (*url.URL, error)
	Recorder  Recorder
	Now       func() time.Time
}

// Execute authorizes and performs one HTTP request.
func (e *HTTPExecutor) Execute(
	ctx context.Context,
	capability model.Capability,
	agentProfile string,
	request HTTPRequest,
) (response HTTPResponse, retErr error) {
	start := time.Now()
	now := time.Now
	if e.Now != nil {
		now = e.Now
	}
	normalized, requestURL, err := AuthorizeHTTP(capability, agentProfile, now(), request.Method, request.URL, int64(len(request.Body)))
	if err != nil {
		return response, err
	}
	if err := validateTransientHeaders(request.Headers); err != nil {
		return response, err
	}
	redirects := 0
	errorClass := "none"
	defer func() {
		if retErr != nil {
			errorClass = ClassifyConnectionError(retErr)
		}
		auditErr := e.Recorder.Record(context.Background(), normalized, AuditEvent{
			Operation: strings.ToUpper(request.Method), Target: request.URL,
			Success: retErr == nil, ErrorClass: errorClass, Duration: time.Since(start),
			RequestBytes: int64(len(request.Body)), ResponseBytes: int64(len(response.Body)), Redirects: redirects,
		})
		if auditErr != nil && retErr == nil {
			retErr = fmt.Errorf("record capability audit: %w", auditErr)
		}
	}()

	timeoutCtx, cancel := context.WithTimeout(ctx, time.Duration(normalized.Limits.TimeoutSeconds)*time.Second)
	defer cancel()
	httpRequest, err := http.NewRequestWithContext(timeoutCtx, strings.ToUpper(request.Method), requestURL.String(), bytes.NewReader(request.Body))
	if err != nil {
		return response, err
	}
	for name, values := range request.Headers {
		for _, value := range values {
			httpRequest.Header.Add(name, value)
		}
	}

	proxy := e.Proxy
	if proxy == nil {
		proxy = http.ProxyFromEnvironment
	}
	transport := e.Transport
	if transport == nil {
		clone := http.DefaultTransport.(*http.Transport).Clone()
		clone.Proxy = proxy
		clone.MaxResponseHeaderBytes = maxTransientHeaderBytes
		transport = clone
	}
	response.Diagnostics = HTTPDiagnostics{Proxy: sanitizedProxy(proxy, httpRequest), CAStore: "system"}
	client := &http.Client{
		Transport: transport,
		CheckRedirect: func(next *http.Request, via []*http.Request) error {
			redirects = len(via)
			if redirects > normalized.Limits.MaxRedirects {
				return ErrDenied{Reason: "HTTP redirect limit exceeded"}
			}
			if _, _, authErr := AuthorizeHTTP(normalized, agentProfile, now(), next.Method, next.URL.String(), int64(len(request.Body))); authErr != nil {
				return fmt.Errorf("redirect rejected: %w", authErr)
			}
			stripSensitiveRedirectHeaders(next.Header)
			return nil
		},
	}
	httpResponse, err := client.Do(httpRequest)
	if err != nil {
		return response, err
	}
	defer httpResponse.Body.Close()
	body, err := io.ReadAll(io.LimitReader(httpResponse.Body, normalized.Limits.MaxResponseBytes+1))
	if err != nil {
		return response, err
	}
	if int64(len(body)) > normalized.Limits.MaxResponseBytes {
		return response, responseLimitError{}
	}
	response.StatusCode = httpResponse.StatusCode
	response.Status = httpResponse.Status
	response.Headers = cloneHeaders(httpResponse.Header)
	response.Body = body
	response.EffectiveURL = httpResponse.Request.URL.String()
	response.Redirects = redirects
	if httpResponse.StatusCode == http.StatusProxyAuthRequired {
		return response, proxyPolicyError{}
	}
	return response, nil
}

func validateTransientHeaders(headers map[string][]string) error {
	total := 0
	for name, values := range headers {
		canonical := http.CanonicalHeaderKey(name)
		if canonical == "" || strings.ContainsAny(name, "\r\n") || strings.EqualFold(canonical, "Host") || strings.EqualFold(canonical, "Proxy-Authorization") || isHopByHopHeader(canonical) {
			return ErrDenied{Reason: fmt.Sprintf("HTTP header %q is not allowed", name)}
		}
		for _, value := range values {
			if strings.ContainsAny(value, "\r\n") {
				return ErrDenied{Reason: fmt.Sprintf("HTTP header %q contains a newline", name)}
			}
			total += len(name) + len(value)
		}
	}
	if total > maxTransientHeaderBytes {
		return ErrDenied{Reason: "HTTP headers exceed their byte limit"}
	}
	return nil
}

func isHopByHopHeader(name string) bool {
	return contains([]string{"Connection", "Keep-Alive", "Proxy-Connection", "Te", "Trailer", "Transfer-Encoding", "Upgrade"}, name)
}

func stripSensitiveRedirectHeaders(headers http.Header) {
	for _, name := range []string{"Authorization", "Cookie", "Proxy-Authorization"} {
		headers.Del(name)
	}
}

func sanitizedProxy(proxy func(*http.Request) (*url.URL, error), request *http.Request) string {
	proxyURL, err := proxy(request)
	if err != nil {
		return "error"
	}
	if proxyURL == nil {
		return "direct"
	}
	copyURL := *proxyURL
	copyURL.User = nil
	copyURL.RawQuery = ""
	copyURL.Fragment = ""
	return copyURL.String()
}

func cloneHeaders(headers http.Header) map[string][]string {
	cloned := make(map[string][]string, len(headers))
	for name, values := range headers {
		cloned[name] = append([]string(nil), values...)
	}
	return cloned
}

type responseLimitError struct{}

func (responseLimitError) Error() string { return "HTTP response exceeds its byte limit" }

type proxyPolicyError struct{}

func (proxyPolicyError) Error() string { return "HTTP proxy requires authentication" }

// ClassifyConnectionError maps dependency errors to stable, audit-safe classes.
func ClassifyConnectionError(err error) string {
	if err == nil {
		return "none"
	}
	var denied ErrDenied
	if errors.As(err, &denied) {
		return "denied"
	}
	var responseLimit responseLimitError
	if errors.As(err, &responseLimit) {
		return "response-limit"
	}
	var proxyPolicy proxyPolicyError
	if errors.As(err, &proxyPolicy) {
		return "proxy"
	}
	if errors.Is(err, context.DeadlineExceeded) {
		return "timeout"
	}
	if errors.Is(err, context.Canceled) {
		return "cancelled"
	}
	var dns *net.DNSError
	if errors.As(err, &dns) {
		return "dns"
	}
	var unknownAuthority x509.UnknownAuthorityError
	if errors.As(err, &unknownAuthority) {
		return "tls"
	}
	var certificateInvalid x509.CertificateInvalidError
	if errors.As(err, &certificateInvalid) {
		return "tls"
	}
	if errors.Is(err, syscall.ENETUNREACH) || errors.Is(err, syscall.EHOSTUNREACH) || errors.Is(err, syscall.ENETDOWN) || errors.Is(err, syscall.ECONNREFUSED) {
		return "routing"
	}
	if errors.Is(err, os.ErrPermission) || errors.Is(err, syscall.EPERM) {
		return "sandbox"
	}
	var urlError *url.Error
	if errors.As(err, &urlError) && urlError.Err != err {
		return ClassifyConnectionError(urlError.Err)
	}
	return "transport"
}
