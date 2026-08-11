package updater

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync/atomic"
	"testing"
	"time"
)

func TestCheckSelectsExactPackagedAsset(t *testing.T) {
	t.Parallel()
	tests := []struct {
		name   string
		target Target
		asset  string
	}{
		{name: "mac Intel", target: Target{GOOS: "darwin", GOARCH: "amd64"}, asset: "p-track_1.2.4_darwin_amd64.dmg"},
		{name: "mac Apple silicon", target: Target{GOOS: "darwin", GOARCH: "arm64"}, asset: "p-track_1.2.4_darwin_arm64.dmg"},
		{name: "Linux Intel", target: Target{GOOS: "linux", GOARCH: "amd64"}, asset: "ptrack_1.2.4_linux_amd64.tar.gz"},
		{name: "Linux ARM", target: Target{GOOS: "linux", GOARCH: "arm64"}, asset: "ptrack_1.2.4_linux_arm64.tar.gz"},
		{name: "Windows Intel", target: Target{GOOS: "windows", GOARCH: "amd64"}, asset: "ptrack_1.2.4_windows_amd64.zip"},
		{name: "Windows ARM", target: Target{GOOS: "windows", GOARCH: "arm64"}, asset: "ptrack_1.2.4_windows_arm64.zip"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			t.Parallel()
			client := fixtureClient(t, releaseJSON("v1.2.4", false, false, tt.asset))
			candidate, err := client.Check(context.Background(), "v1.2.3", tt.target)
			if err != nil {
				t.Fatal(err)
			}
			if candidate.Version != "1.2.4" || candidate.Tag != "v1.2.4" || candidate.Package.Name != tt.asset {
				t.Fatalf("candidate = %#v", candidate)
			}
			if candidate.Checksums.Name != "checksums.txt" || candidate.PageURL != "https://github.com/ro-ag/ptrack/releases/tag/v1.2.4" {
				t.Fatalf("candidate metadata = %#v", candidate)
			}
		})
	}
}

func TestCheckRejectsDevelopmentUnsupportedAndNonNewerVersions(t *testing.T) {
	t.Parallel()
	client := fixtureClient(t, releaseJSON("v1.2.4", false, false, "ptrack_1.2.4_linux_arm64.tar.gz"))
	for _, current := range []string{"dev", "", "1.2", "1.2.3-beta.1", "01.2.3", "vv1.2.3"} {
		if _, err := client.Check(context.Background(), current, Target{GOOS: "linux", GOARCH: "arm64"}); !errors.Is(err, ErrDevelopmentBuild) {
			t.Errorf("current %q: error = %v, want ErrDevelopmentBuild", current, err)
		}
	}
	for _, current := range []string{"1.2.4", "1.2.5", "2.0.0"} {
		if _, err := client.Check(context.Background(), current, Target{GOOS: "linux", GOARCH: "arm64"}); !errors.Is(err, ErrNoUpdate) {
			t.Errorf("current %q: error = %v, want ErrNoUpdate", current, err)
		}
	}
	for _, target := range []Target{{GOOS: "freebsd", GOARCH: "amd64"}, {GOOS: "linux", GOARCH: "386"}, {GOOS: "", GOARCH: ""}} {
		if _, err := client.Check(context.Background(), "1.2.3", target); !errors.Is(err, ErrUnsupportedTarget) {
			t.Errorf("target %#v: error = %v, want ErrUnsupportedTarget", target, err)
		}
	}
}

func TestCheckRejectsUnsafeReleaseMetadata(t *testing.T) {
	t.Parallel()
	asset := "ptrack_1.2.4_linux_amd64.tar.gz"
	valid := releaseJSON("v1.2.4", false, false, asset)
	tests := []struct {
		name string
		body string
	}{
		{name: "draft", body: releaseJSON("v1.2.4", true, false, asset)},
		{name: "prerelease flag", body: releaseJSON("v1.2.4", false, true, asset)},
		{name: "prerelease tag", body: releaseJSON("v1.2.4-rc.1", false, false, asset)},
		{name: "missing package", body: strings.ReplaceAll(valid, asset, "ptrack_1.2.4_linux_arm64.tar.gz")},
		{name: "duplicate package", body: duplicateAsset(valid, asset)},
		{name: "duplicate checksum", body: duplicateAsset(valid, "checksums.txt")},
		{name: "wrong package host", body: strings.Replace(valid, "https://github.com/ro-ag/ptrack/releases/download/v1.2.4/"+asset, "https://evil.example/"+asset, 1)},
		{name: "wrong package path", body: strings.Replace(valid, "/ro-ag/ptrack/releases/download/", "/ro-ag/other/releases/download/", 1)},
		{name: "package query", body: strings.Replace(valid, asset+`","size"`, asset+`?token=secret","size"`, 1)},
		{name: "empty package query", body: strings.Replace(valid, asset+`","size"`, asset+`?","size"`, 1)},
		{name: "package fragment", body: strings.Replace(valid, asset+`","size"`, asset+`#fragment","size"`, 1)},
		{name: "empty package fragment", body: strings.Replace(valid, asset+`","size"`, asset+`#","size"`, 1)},
		{name: "package port", body: strings.Replace(valid, "https://github.com/", "https://github.com:443/", 1)},
		{name: "package userinfo", body: strings.Replace(valid, "https://github.com/", "https://user@github.com/", 1)},
		{name: "wrong checksum path", body: strings.Replace(valid, "/checksums.txt", "/other.txt", 1)},
		{name: "pending asset", body: strings.Replace(valid, `"state":"uploaded"`, `"state":"new"`, 1)},
		{name: "empty asset", body: strings.Replace(valid, `"size":1024`, `"size":0`, 1)},
		{name: "oversized package", body: strings.Replace(valid, `"size":1024`, fmt.Sprintf(`"size":%d`, maxAssetBytes+1), 1)},
		{name: "oversized checksum", body: strings.Replace(valid, `"size":128`, fmt.Sprintf(`"size":%d`, maxManifestBytes+1), 1)},
		{name: "missing published time", body: strings.Replace(valid, `"published_at":"2026-08-11T00:00:00Z"`, `"published_at":"0001-01-01T00:00:00Z"`, 1)},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			t.Parallel()
			client := fixtureClient(t, tt.body)
			if _, err := client.Check(context.Background(), "1.2.3", Target{GOOS: "linux", GOARCH: "amd64"}); !errors.Is(err, ErrInvalidRelease) {
				t.Fatalf("error = %v, want ErrInvalidRelease", err)
			}
		})
	}
}

func TestCheckIgnoresGitHubSourceArchiveFields(t *testing.T) {
	t.Parallel()
	asset := "ptrack_1.2.4_linux_amd64.tar.gz"
	body := strings.Replace(releaseJSON("v1.2.4", false, false, asset), `"assets":`, `"tarball_url":"https://api.github.com/repos/ro-ag/ptrack/tarball/v1.2.4","zipball_url":"https://api.github.com/repos/ro-ag/ptrack/zipball/v1.2.4","assets":`, 1)
	client := fixtureClient(t, body)
	candidate, err := client.Check(context.Background(), "1.2.3", Target{GOOS: "linux", GOARCH: "amd64"})
	if err != nil {
		t.Fatal(err)
	}
	if candidate.Package.Name != asset || strings.Contains(candidate.Package.DownloadURL, "tarball") {
		t.Fatalf("source archive selected: %#v", candidate.Package)
	}
}

func TestCheckBoundsAndCancelsMetadataRequest(t *testing.T) {
	t.Parallel()
	t.Run("oversized", func(t *testing.T) {
		t.Parallel()
		client := fixtureClient(t, strings.Repeat("x", maxMetadataBytes+1))
		if _, err := client.Check(context.Background(), "1.2.3", Target{GOOS: "linux", GOARCH: "amd64"}); !errors.Is(err, ErrInvalidRelease) {
			t.Fatalf("error = %v, want ErrInvalidRelease", err)
		}
	})
	t.Run("canceled", func(t *testing.T) {
		t.Parallel()
		started := make(chan struct{})
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			close(started)
			<-r.Context().Done()
		}))
		defer server.Close()
		client := &Client{endpoint: server.URL, client: server.Client()}
		ctx, cancel := context.WithCancel(context.Background())
		done := make(chan error, 1)
		go func() {
			_, err := client.Check(ctx, "1.2.3", Target{GOOS: "linux", GOARCH: "amd64"})
			done <- err
		}()
		<-started
		cancel()
		if err := <-done; !errors.Is(err, context.Canceled) {
			t.Fatalf("error = %v, want context.Canceled", err)
		}
	})
	t.Run("client timeout", func(t *testing.T) {
		t.Parallel()
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			<-r.Context().Done()
		}))
		defer server.Close()
		client := &Client{endpoint: server.URL, client: &http.Client{Timeout: 20 * time.Millisecond}}
		if _, err := client.Check(context.Background(), "1.2.3", Target{GOOS: "linux", GOARCH: "amd64"}); !errors.Is(err, context.DeadlineExceeded) {
			t.Fatalf("error = %v, want deadline exceeded", err)
		}
	})
}

func TestCheckSendsBoundedGitHubRequestHeaders(t *testing.T) {
	t.Parallel()
	asset := "ptrack_1.2.4_linux_amd64.tar.gz"
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodGet || r.Header.Get("User-Agent") != userAgent ||
			r.Header.Get("Accept") != "application/vnd.github+json" ||
			r.Header.Get("X-GitHub-Api-Version") != "2022-11-28" {
			t.Errorf("unexpected request: method=%s headers=%v", r.Method, r.Header)
		}
		fmt.Fprint(w, releaseJSON("v1.2.4", false, false, asset))
	}))
	defer server.Close()
	client := &Client{endpoint: server.URL, client: server.Client()}
	if _, err := client.Check(context.Background(), "1.2.3", Target{GOOS: "linux", GOARCH: "amd64"}); err != nil {
		t.Fatal(err)
	}
}

func TestCheckRejectsMetadataRedirectAndStatus(t *testing.T) {
	t.Parallel()
	var destinationRequests atomic.Int32
	final := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		destinationRequests.Add(1)
		fmt.Fprint(w, `{}`)
	}))
	defer final.Close()
	redirect := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		http.Redirect(w, r, final.URL, http.StatusFound)
	}))
	defer redirect.Close()
	client := &Client{endpoint: redirect.URL, client: redirect.Client()}
	if _, err := client.Check(context.Background(), "1.2.3", Target{GOOS: "linux", GOARCH: "amd64"}); !errors.Is(err, ErrInvalidRelease) {
		t.Fatalf("redirect error = %v, want ErrInvalidRelease", err)
	}
	if got := destinationRequests.Load(); got != 0 {
		t.Fatalf("redirect destination received %d requests, want 0", got)
	}

	status := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusTooManyRequests)
	}))
	defer status.Close()
	client = &Client{endpoint: status.URL, client: status.Client()}
	if _, err := client.Check(context.Background(), "1.2.3", Target{GOOS: "linux", GOARCH: "amd64"}); err == nil || errors.Is(err, ErrInvalidRelease) {
		t.Fatalf("status error = %v", err)
	}
}

func fixtureClient(t *testing.T, body string) *Client {
	t.Helper()
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		fmt.Fprint(w, body)
	}))
	t.Cleanup(server.Close)
	return &Client{endpoint: server.URL, client: server.Client()}
}

func releaseJSON(tag string, draft, prerelease bool, asset string) string {
	return fmt.Sprintf(`{
  "tag_name":%q,
  "body":"Safe release notes",
  "draft":%t,
  "prerelease":%t,
  "published_at":"2026-08-11T00:00:00Z",
  "assets":[
    {"name":%q,"browser_download_url":%q,"size":1024,"state":"uploaded"},
    {"name":"checksums.txt","browser_download_url":%q,"size":128,"state":"uploaded"}
  ]
}`,
		tag,
		draft,
		prerelease,
		asset,
		"https://github.com/ro-ag/ptrack/releases/download/"+tag+"/"+asset,
		"https://github.com/ro-ag/ptrack/releases/download/"+tag+"/checksums.txt",
	)
}

func duplicateAsset(body, name string) string {
	needle := fmt.Sprintf(`{"name":%q`, name)
	start := strings.Index(body, needle)
	if start < 0 {
		return body
	}
	end := strings.Index(body[start:], "}")
	if end < 0 {
		return body
	}
	asset := body[start : start+end+1]
	return strings.Replace(body, `"assets":[`, `"assets":[`+asset+`,`, 1)
}

func TestParseVersionStrictness(t *testing.T) {
	t.Parallel()
	for _, valid := range []string{"0.0.0", "1.2.3", "v1.2.3", "18446744073709551615.0.1"} {
		if _, err := parseVersion(valid, true); err != nil {
			t.Errorf("parseVersion(%q) = %v", valid, err)
		}
	}
	for _, invalid := range []string{"v", "1", "1.2", "1.2.3.4", "1.02.3", "-1.2.3", "1.2.3+meta", "1.2.3-rc.1", " 1.2.3", "1.2.3 ", "18446744073709551616.0.0"} {
		if _, err := parseVersion(invalid, true); err == nil {
			t.Errorf("parseVersion(%q) unexpectedly succeeded", invalid)
		}
	}
}

func TestCompareVersions(t *testing.T) {
	t.Parallel()
	for _, test := range []struct {
		left, right string
		want        int
	}{
		{left: "v1.2.4", right: "1.2.3", want: 1},
		{left: "1.2.3", right: "v1.2.3", want: 0},
		{left: "1.2.2", right: "1.2.3", want: -1},
	} {
		got, err := CompareVersions(test.left, test.right)
		if err != nil || got != test.want {
			t.Errorf("CompareVersions(%q, %q) = %d, %v; want %d", test.left, test.right, got, err, test.want)
		}
	}
	if _, err := CompareVersions("dev", "1.2.3"); err == nil {
		t.Fatal("CompareVersions accepted dev")
	}
}

func TestNewClientHasProductionSafetyDefaults(t *testing.T) {
	t.Parallel()
	client := NewClient()
	if client.endpoint != latestReleaseURL || client.client.Timeout != 15*time.Second || client.client.CheckRedirect == nil {
		t.Fatalf("unsafe production client: %#v", client)
	}
}
