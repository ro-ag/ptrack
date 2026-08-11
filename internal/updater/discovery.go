// Package updater discovers, verifies, and applies p-track releases.
package updater

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"time"
)

const (
	latestReleaseURL = "https://api.github.com/repos/ro-ag/ptrack/releases/latest"
	userAgent        = "p-track-updater"
	maxMetadataBytes = 1 << 20
	maxNotesBytes    = 32 << 10
	maxManifestBytes = 1 << 20
	maxAssetBytes    = 512 << 20
)

var (
	ErrDevelopmentBuild  = errors.New("updates are unavailable for development builds")
	ErrInvalidRelease    = errors.New("invalid GitHub release")
	ErrNoUpdate          = errors.New("no newer release is available")
	ErrUnsupportedTarget = errors.New("unsupported update target")
)

// Target is a host-selected operating system and architecture pair. It must
// never be populated from frontend or release metadata.
type Target struct {
	GOOS   string
	GOARCH string
}

// Asset is one exact packaged GitHub Release asset selected by the host.
type Asset struct {
	Name        string
	DownloadURL string
	SizeBytes   int64
}

// Candidate is a newer stable release and the two assets required to stage it.
type Candidate struct {
	Version     string
	Tag         string
	PageURL     string
	PublishedAt time.Time
	Notes       string
	Package     Asset
	Checksums   Asset
}

// Client performs bounded discovery against p-track's fixed GitHub repository.
type Client struct {
	endpoint string
	client   *http.Client
}

// NewClient returns a production release-discovery client. Redirects are not
// accepted for metadata; release asset redirects are handled during staging.
func NewClient() *Client {
	return &Client{
		endpoint: latestReleaseURL,
		client: &http.Client{
			Timeout: 15 * time.Second,
			CheckRedirect: func(_ *http.Request, _ []*http.Request) error {
				return http.ErrUseLastResponse
			},
		},
	}
}

// Check returns a strictly newer stable release for target. It accepts only
// p-track's exact packaged asset names and checksum manifest.
func (c *Client) Check(ctx context.Context, currentVersion string, target Target) (Candidate, error) {
	current, err := parseVersion(currentVersion, true)
	if err != nil {
		return Candidate{}, ErrDevelopmentBuild
	}
	wantedName, err := packageName(target, "VERSION")
	if err != nil {
		return Candidate{}, err
	}

	release, err := c.fetch(ctx)
	if err != nil {
		return Candidate{}, err
	}
	remote, err := parseVersion(release.TagName, false)
	if err != nil || release.TagName != "v"+remote.String() || release.Draft || release.Prerelease {
		return Candidate{}, fmt.Errorf("%w: release must be a published stable vX.Y.Z tag", ErrInvalidRelease)
	}
	if remote.compare(current) <= 0 {
		return Candidate{}, ErrNoUpdate
	}
	if release.PublishedAt.IsZero() || len(release.Body) > maxNotesBytes {
		return Candidate{}, fmt.Errorf("%w: missing publication time or oversized notes", ErrInvalidRelease)
	}

	wantedName = strings.Replace(wantedName, "VERSION", remote.String(), 1)
	packageAsset, err := selectAsset(release.Assets, release.TagName, wantedName, maxAssetBytes)
	if err != nil {
		return Candidate{}, err
	}
	checksums, err := selectAsset(release.Assets, release.TagName, "checksums.txt", maxManifestBytes)
	if err != nil {
		return Candidate{}, err
	}

	return Candidate{
		Version:     remote.String(),
		Tag:         release.TagName,
		PageURL:     "https://github.com/ro-ag/ptrack/releases/tag/" + release.TagName,
		PublishedAt: release.PublishedAt.UTC(),
		Notes:       release.Body,
		Package:     packageAsset,
		Checksums:   checksums,
	}, nil
}

type githubRelease struct {
	TagName     string        `json:"tag_name"`
	Body        string        `json:"body"`
	Draft       bool          `json:"draft"`
	Prerelease  bool          `json:"prerelease"`
	PublishedAt time.Time     `json:"published_at"`
	Assets      []githubAsset `json:"assets"`
}

type githubAsset struct {
	Name               string `json:"name"`
	BrowserDownloadURL string `json:"browser_download_url"`
	Size               int64  `json:"size"`
	State              string `json:"state"`
}

func (c *Client) fetch(ctx context.Context) (githubRelease, error) {
	if c == nil || c.client == nil || c.endpoint == "" {
		return githubRelease{}, fmt.Errorf("%w: release client is not configured", ErrInvalidRelease)
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, c.endpoint, nil)
	if err != nil {
		return githubRelease{}, fmt.Errorf("build release request: %w", err)
	}
	req.Header.Set("Accept", "application/vnd.github+json")
	req.Header.Set("User-Agent", userAgent)
	req.Header.Set("X-GitHub-Api-Version", "2022-11-28")

	httpClient := *c.client
	httpClient.CheckRedirect = func(_ *http.Request, _ []*http.Request) error {
		return http.ErrUseLastResponse
	}
	resp, err := httpClient.Do(req)
	if err != nil {
		return githubRelease{}, fmt.Errorf("fetch GitHub release: %w", err)
	}
	defer resp.Body.Close()
	if resp.Request == nil || resp.Request.URL == nil || resp.Request.URL.String() != c.endpoint {
		return githubRelease{}, fmt.Errorf("%w: metadata redirect refused", ErrInvalidRelease)
	}
	if resp.StatusCode >= http.StatusMultipleChoices && resp.StatusCode < http.StatusBadRequest {
		return githubRelease{}, fmt.Errorf("%w: metadata redirect refused", ErrInvalidRelease)
	}
	if resp.StatusCode != http.StatusOK {
		return githubRelease{}, fmt.Errorf("fetch GitHub release: unexpected HTTP status %d", resp.StatusCode)
	}
	body, err := io.ReadAll(io.LimitReader(resp.Body, maxMetadataBytes+1))
	if err != nil {
		return githubRelease{}, fmt.Errorf("read GitHub release: %w", err)
	}
	if len(body) > maxMetadataBytes {
		return githubRelease{}, fmt.Errorf("%w: metadata exceeds %d bytes", ErrInvalidRelease, maxMetadataBytes)
	}
	var release githubRelease
	if err := json.Unmarshal(body, &release); err != nil {
		return githubRelease{}, fmt.Errorf("%w: decode metadata: %v", ErrInvalidRelease, err)
	}
	return release, nil
}

func packageName(target Target, version string) (string, error) {
	if target.GOARCH != "amd64" && target.GOARCH != "arm64" {
		return "", fmt.Errorf("%w: %s/%s", ErrUnsupportedTarget, target.GOOS, target.GOARCH)
	}
	switch target.GOOS {
	case "darwin":
		return fmt.Sprintf("p-track_%s_darwin_%s.dmg", version, target.GOARCH), nil
	case "linux":
		return fmt.Sprintf("ptrack_%s_linux_%s.tar.gz", version, target.GOARCH), nil
	case "windows":
		return fmt.Sprintf("ptrack_%s_windows_%s.zip", version, target.GOARCH), nil
	default:
		return "", fmt.Errorf("%w: %s/%s", ErrUnsupportedTarget, target.GOOS, target.GOARCH)
	}
}

func selectAsset(assets []githubAsset, tag, name string, maxSize int64) (Asset, error) {
	var selected *githubAsset
	for i := range assets {
		if assets[i].Name != name {
			continue
		}
		if selected != nil {
			return Asset{}, fmt.Errorf("%w: duplicate asset %q", ErrInvalidRelease, name)
		}
		selected = &assets[i]
	}
	if selected == nil {
		return Asset{}, fmt.Errorf("%w: missing asset %q", ErrInvalidRelease, name)
	}
	if selected.State != "uploaded" || selected.Size <= 0 || selected.Size > maxSize {
		return Asset{}, fmt.Errorf("%w: invalid asset %q", ErrInvalidRelease, name)
	}
	if err := validateAssetURL(selected.BrowserDownloadURL, tag, name); err != nil {
		return Asset{}, err
	}
	return Asset{Name: name, DownloadURL: selected.BrowserDownloadURL, SizeBytes: selected.Size}, nil
}

func validateAssetURL(rawURL, tag, name string) error {
	u, err := url.Parse(rawURL)
	if err != nil || u.Scheme != "https" || u.Host != "github.com" || u.User != nil ||
		u.RawQuery != "" || u.ForceQuery || u.Fragment != "" || strings.ContainsAny(rawURL, "?#") {
		return fmt.Errorf("%w: unsafe URL for asset %q", ErrInvalidRelease, name)
	}
	wantPath := "/ro-ag/ptrack/releases/download/" + tag + "/" + name
	if u.EscapedPath() != wantPath || u.Path != wantPath {
		return fmt.Errorf("%w: unexpected URL for asset %q", ErrInvalidRelease, name)
	}
	return nil
}

type semVersion struct {
	major uint64
	minor uint64
	patch uint64
}

func parseVersion(raw string, allowOptionalV bool) (semVersion, error) {
	if allowOptionalV {
		raw = strings.TrimPrefix(raw, "v")
	} else if !strings.HasPrefix(raw, "v") {
		return semVersion{}, errors.New("version must start with v")
	} else {
		raw = strings.TrimPrefix(raw, "v")
	}
	parts := strings.Split(raw, ".")
	if len(parts) != 3 {
		return semVersion{}, errors.New("version must have three components")
	}
	values := make([]uint64, 3)
	for i, part := range parts {
		if part == "" || (len(part) > 1 && part[0] == '0') {
			return semVersion{}, errors.New("invalid numeric version component")
		}
		value, err := strconv.ParseUint(part, 10, 64)
		if err != nil {
			return semVersion{}, errors.New("invalid numeric version component")
		}
		values[i] = value
	}
	return semVersion{major: values[0], minor: values[1], patch: values[2]}, nil
}

func (v semVersion) String() string {
	return fmt.Sprintf("%d.%d.%d", v.major, v.minor, v.patch)
}

func (v semVersion) compare(other semVersion) int {
	left := [...]uint64{v.major, v.minor, v.patch}
	right := [...]uint64{other.major, other.minor, other.patch}
	for i := range left {
		if left[i] < right[i] {
			return -1
		}
		if left[i] > right[i] {
			return 1
		}
	}
	return 0
}

// CompareVersions compares two strict release versions (with optional leading
// v) and returns -1, 0, or 1. Development and prerelease strings are rejected.
func CompareVersions(left, right string) (int, error) {
	leftVersion, err := parseVersion(left, true)
	if err != nil {
		return 0, err
	}
	rightVersion, err := parseVersion(right, true)
	if err != nil {
		return 0, err
	}
	return leftVersion.compare(rightVersion), nil
}
