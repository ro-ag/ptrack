package updater

import (
	"archive/tar"
	"archive/zip"
	"bytes"
	"compress/gzip"
	"context"
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"testing"
	"time"
)

func TestLiveLatestReleaseContract(t *testing.T) {
	if os.Getenv("PTRACK_LIVE_UPDATE_TEST") != "1" {
		t.Skip("set PTRACK_LIVE_UPDATE_TEST=1 to exercise the published GitHub Release")
	}
	target := Target{GOOS: runtime.GOOS, GOARCH: runtime.GOARCH}
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
	defer cancel()
	client := NewClient()
	candidate, err := client.Check(ctx, "0.0.0", target)
	if err != nil {
		t.Fatal(err)
	}
	stage, err := client.Stage(ctx, candidate, target, t.TempDir(), nil)
	if err != nil {
		t.Fatal(err)
	}
	if err := ValidateStage(stage); err != nil {
		t.Fatal(err)
	}
}

func TestStageVerifiesAndValidatesEveryPlatformPayload(t *testing.T) {
	t.Parallel()
	tests := []struct {
		name    string
		target  Target
		kind    StageKind
		payload []byte
		archive func(*testing.T, Target, []byte) []byte
	}{
		{name: "Linux Intel", target: Target{GOOS: "linux", GOARCH: "amd64"}, kind: StageLinuxBinary, payload: fakeELF(t, "amd64"), archive: tarRelease},
		{name: "Linux ARM", target: Target{GOOS: "linux", GOARCH: "arm64"}, kind: StageLinuxBinary, payload: fakeELF(t, "arm64"), archive: tarRelease},
		{name: "Windows Intel", target: Target{GOOS: "windows", GOARCH: "amd64"}, kind: StageWindowsZIP, payload: fakePE(t, "amd64"), archive: zipRelease},
		{name: "Windows ARM", target: Target{GOOS: "windows", GOARCH: "arm64"}, kind: StageWindowsZIP, payload: fakePE(t, "arm64"), archive: zipRelease},
		{name: "macOS checksum-only DMG stage", target: Target{GOOS: "darwin", GOARCH: "arm64"}, kind: StageDarwinDMG, payload: []byte("synthetic disk image"), archive: rawRelease},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			t.Parallel()
			packageBytes := tt.archive(t, tt.target, tt.payload)
			client, candidate, requests := stageFixture(t, tt.target, packageBytes)
			var progress []Progress
			stage, err := client.Stage(context.Background(), candidate, tt.target, t.TempDir(), func(item Progress) {
				progress = append(progress, item)
			})
			if err != nil {
				t.Fatal(err)
			}
			t.Cleanup(func() { _ = os.RemoveAll(stage.Root) })
			if stage.Kind != tt.kind || stage.Version != "1.2.4" || stage.GOOS != tt.target.GOOS || stage.GOARCH != tt.target.GOARCH {
				t.Fatalf("stage = %#v", stage)
			}
			if err := ValidateStage(stage); err != nil {
				t.Fatalf("ValidateStage: %v", err)
			}
			loaded, err := LoadStage(stage.Root)
			if err != nil || loaded != stage {
				t.Fatalf("LoadStage = %#v, %v; want %#v", loaded, err, stage)
			}
			if tt.kind != StageDarwinDMG {
				got, err := os.ReadFile(stage.PayloadPath)
				if err != nil || !bytes.Equal(got, tt.payload) {
					t.Fatalf("payload mismatch: err=%v bytes=%x", err, got)
				}
			}
			if len(progress) < 2 || progress[len(progress)-1].Asset != "package" ||
				progress[len(progress)-1].Downloaded != int64(len(packageBytes)) {
				t.Fatalf("progress = %#v", progress)
			}
			if requests.count("github.com") != 2 || requests.count("release-assets.githubusercontent.com") != 2 {
				t.Fatalf("requests = %#v", requests.hosts)
			}
			if runtime.GOOS != "windows" {
				if info, err := os.Stat(stage.Root); err != nil || info.Mode().Perm() != 0o700 {
					t.Fatalf("stage permissions = %v err=%v", info.Mode().Perm(), err)
				}
			}
		})
	}
}

func TestValidateStageRejectsTamperingAndEscapingPaths(t *testing.T) {
	t.Parallel()
	target := Target{GOOS: "linux", GOARCH: "amd64"}
	client, candidate, _ := stageFixture(t, target, tarRelease(t, target, fakeELF(t, "amd64")))
	stage, err := client.Stage(context.Background(), candidate, target, t.TempDir(), nil)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.RemoveAll(stage.Root) })

	t.Run("package changed", func(t *testing.T) {
		copy := stage
		if err := os.WriteFile(copy.AssetPath, []byte("tampered"), 0o600); err != nil {
			t.Fatal(err)
		}
		if err := ValidateStage(copy); !errors.Is(err, ErrInvalidStage) {
			t.Fatalf("error = %v, want ErrInvalidStage", err)
		}
	})

	t.Run("path escape", func(t *testing.T) {
		copy := stage
		copy.AssetPath = filepath.Join(stage.Root, "..", "outside")
		if err := ValidateStage(copy); !errors.Is(err, ErrInvalidStage) {
			t.Fatalf("error = %v, want ErrInvalidStage", err)
		}
	})
}

func TestValidateStageRejectsPayloadSubstitutionAndSymlink(t *testing.T) {
	t.Parallel()
	target := Target{GOOS: "linux", GOARCH: "amd64"}
	makeStage := func(t *testing.T) StagedUpdate {
		t.Helper()
		client, candidate, _ := stageFixture(t, target, tarRelease(t, target, fakeELF(t, "amd64")))
		stage, err := client.Stage(context.Background(), candidate, target, t.TempDir(), nil)
		if err != nil {
			t.Fatal(err)
		}
		return stage
	}

	t.Run("same-machine replacement", func(t *testing.T) {
		t.Parallel()
		stage := makeStage(t)
		replacement := append(fakeELF(t, "amd64"), byte(1))
		if err := os.WriteFile(stage.PayloadPath, replacement, 0o600); err != nil {
			t.Fatal(err)
		}
		if err := ValidateStage(stage); !errors.Is(err, ErrInvalidStage) {
			t.Fatalf("error = %v, want ErrInvalidStage", err)
		}
	})

	if runtime.GOOS != "windows" {
		t.Run("symlink replacement", func(t *testing.T) {
			t.Parallel()
			stage := makeStage(t)
			outside := filepath.Join(t.TempDir(), "outside")
			if err := os.WriteFile(outside, fakeELF(t, "amd64"), 0o600); err != nil {
				t.Fatal(err)
			}
			if err := os.Remove(stage.PayloadPath); err != nil {
				t.Fatal(err)
			}
			if err := os.Symlink(outside, stage.PayloadPath); err != nil {
				t.Fatal(err)
			}
			if err := ValidateStage(stage); !errors.Is(err, ErrInvalidStage) {
				t.Fatalf("error = %v, want ErrInvalidStage", err)
			}
		})
	}
}

func TestValidateStageBoundsPersistedSizesAndHonorsCancellation(t *testing.T) {
	t.Parallel()
	target := Target{GOOS: "darwin", GOARCH: "arm64"}
	client, candidate, _ := stageFixture(t, target, []byte("dmg"))
	stage, err := client.Stage(context.Background(), candidate, target, t.TempDir(), nil)
	if err != nil {
		t.Fatal(err)
	}
	oversized := stage
	oversized.SizeBytes = maxAssetBytes + 1
	if err := ValidateStage(oversized); !errors.Is(err, ErrInvalidStage) {
		t.Fatalf("oversized error = %v, want ErrInvalidStage", err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if err := ValidateStageContext(ctx, stage); !errors.Is(err, context.Canceled) {
		t.Fatalf("canceled error = %v, want context.Canceled", err)
	}
}

func TestLoadStageRejectsUnknownAndTrailingRecordData(t *testing.T) {
	t.Parallel()
	target := Target{GOOS: "darwin", GOARCH: "arm64"}
	client, candidate, _ := stageFixture(t, target, []byte("dmg"))
	stage, err := client.Stage(context.Background(), candidate, target, t.TempDir(), nil)
	if err != nil {
		t.Fatal(err)
	}
	data, err := os.ReadFile(stage.StatePath)
	if err != nil {
		t.Fatal(err)
	}
	data = bytes.Replace(data, []byte(`"kind"`), []byte(`"unknown":true,"kind"`), 1)
	if err := os.WriteFile(stage.StatePath, data, 0o600); err != nil {
		t.Fatal(err)
	}
	if _, err := LoadStage(stage.Root); !errors.Is(err, ErrInvalidStage) {
		t.Fatalf("error = %v, want ErrInvalidStage", err)
	}
}

func TestStageRejectsChecksumAndDownloadFailuresAndCleansPartialState(t *testing.T) {
	t.Parallel()
	target := Target{GOOS: "darwin", GOARCH: "arm64"}
	packageBytes := []byte("dmg")
	tests := []struct {
		name   string
		mutate func(*Candidate, map[string][]byte, *stageTransport)
	}{
		{name: "checksum mismatch", mutate: func(_ *Candidate, bodies map[string][]byte, _ *stageTransport) {
			bodies["checksums.txt"] = []byte(strings.Repeat("0", 64) + "  p-track_1.2.4_darwin_arm64.dmg\n")
		}},
		{name: "duplicate checksum", mutate: func(_ *Candidate, bodies map[string][]byte, _ *stageTransport) {
			bodies["checksums.txt"] = append(bodies["checksums.txt"], bodies["checksums.txt"]...)
		}},
		{name: "missing checksum", mutate: func(_ *Candidate, bodies map[string][]byte, _ *stageTransport) {
			bodies["checksums.txt"] = []byte(strings.Repeat("0", 64) + "  other.dmg\n")
		}},
		{name: "malformed checksum", mutate: func(_ *Candidate, bodies map[string][]byte, _ *stageTransport) {
			bodies["checksums.txt"] = []byte("not-a-checksum\n")
		}},
		{name: "evil redirect", mutate: func(_ *Candidate, _ map[string][]byte, transport *stageTransport) {
			transport.redirectHost = "evil.example"
		}},
		{name: "metadata size mismatch", mutate: func(candidate *Candidate, _ map[string][]byte, _ *stageTransport) {
			candidate.Package.SizeBytes++
		}},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			t.Parallel()
			client, candidate, requests := stageFixture(t, target, packageBytes)
			transport := client.client.Transport.(*stageTransport)
			tt.mutate(&candidate, transport.bodies, transport)
			if body := transport.bodies["checksums.txt"]; int64(len(body)) != candidate.Checksums.SizeBytes {
				candidate.Checksums.SizeBytes = int64(len(body))
			}
			base := t.TempDir()
			if _, err := client.Stage(context.Background(), candidate, target, base, nil); err == nil {
				t.Fatal("Stage unexpectedly succeeded")
			}
			entries, err := os.ReadDir(base)
			if err != nil || len(entries) != 0 {
				t.Fatalf("partial stage retained: entries=%v err=%v", entries, err)
			}
			if tt.name == "evil redirect" && requests.count("evil.example") != 0 {
				t.Fatalf("evil redirect received %d requests", requests.count("evil.example"))
			}
		})
	}
}

func TestStageRejectsUnsafeArchiveLayoutsAndMachines(t *testing.T) {
	t.Parallel()
	t.Run("tar traversal", func(t *testing.T) {
		t.Parallel()
		archive := customTar(t, []tarItem{
			{name: "ptrack", body: fakeELF(t, "amd64"), mode: tar.TypeReg},
			{name: "README.md", body: []byte("readme"), mode: tar.TypeReg},
			{name: "LICENSE", body: []byte("license"), mode: tar.TypeReg},
			{name: "../escape", body: []byte("bad"), mode: tar.TypeReg},
		})
		assertStageRejected(t, Target{GOOS: "linux", GOARCH: "amd64"}, archive)
	})
	t.Run("tar symlink", func(t *testing.T) {
		t.Parallel()
		archive := customTar(t, []tarItem{
			{name: "ptrack", body: nil, mode: tar.TypeSymlink},
			{name: "README.md", body: []byte("readme"), mode: tar.TypeReg},
			{name: "LICENSE", body: []byte("license"), mode: tar.TypeReg},
		})
		assertStageRejected(t, Target{GOOS: "linux", GOARCH: "amd64"}, archive)
	})
	t.Run("tar extra entry", func(t *testing.T) {
		t.Parallel()
		archive := customTar(t, []tarItem{
			{name: "ptrack", body: fakeELF(t, "amd64"), mode: tar.TypeReg},
			{name: "README.md", body: []byte("readme"), mode: tar.TypeReg},
			{name: "LICENSE", body: []byte("license"), mode: tar.TypeReg},
			{name: "install.sh", body: []byte("bad"), mode: tar.TypeReg},
		})
		assertStageRejected(t, Target{GOOS: "linux", GOARCH: "amd64"}, archive)
	})
	t.Run("wrong ELF machine", func(t *testing.T) {
		t.Parallel()
		assertStageRejected(t, Target{GOOS: "linux", GOARCH: "amd64"}, tarRelease(t, Target{}, fakeELF(t, "arm64")))
	})
	t.Run("wrong PE machine", func(t *testing.T) {
		t.Parallel()
		assertStageRejected(t, Target{GOOS: "windows", GOARCH: "amd64"}, zipRelease(t, Target{}, fakePE(t, "arm64")))
	})
	t.Run("raw COFF executable", func(t *testing.T) {
		t.Parallel()
		assertStageRejected(t, Target{GOOS: "windows", GOARCH: "amd64"}, zipRelease(t, Target{}, fakeCOFF()))
	})
	t.Run("zip traversal", func(t *testing.T) {
		t.Parallel()
		archive := customZip(t, []zipItem{
			{name: "ptrack.exe", body: fakePE(t, "amd64")},
			{name: "README.md", body: []byte("readme")},
			{name: "LICENSE", body: []byte("license")},
			{name: "../escape", body: []byte("bad")},
		})
		assertStageRejected(t, Target{GOOS: "windows", GOARCH: "amd64"}, archive)
	})
}

func TestStageRejectsCanceledRequestAndUnsafeBase(t *testing.T) {
	t.Parallel()
	target := Target{GOOS: "darwin", GOARCH: "arm64"}
	client, candidate, _ := stageFixture(t, target, []byte("dmg"))
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if _, err := client.Stage(ctx, candidate, target, t.TempDir(), nil); !errors.Is(err, context.Canceled) {
		t.Fatalf("error = %v, want context.Canceled", err)
	}
	if _, err := client.Stage(context.Background(), candidate, target, "relative", nil); !errors.Is(err, ErrInvalidStage) {
		t.Fatalf("relative base error = %v, want ErrInvalidStage", err)
	}
	if runtime.GOOS != "windows" {
		base := t.TempDir()
		targetDir := filepath.Join(base, "real")
		if err := os.Mkdir(targetDir, 0o700); err != nil {
			t.Fatal(err)
		}
		link := filepath.Join(base, "link")
		if err := os.Symlink(targetDir, link); err != nil {
			t.Fatal(err)
		}
		if _, err := client.Stage(context.Background(), candidate, target, link, nil); !errors.Is(err, ErrInvalidStage) {
			t.Fatalf("symlink base error = %v, want ErrInvalidStage", err)
		}
	}
}

type stageRequests struct {
	mu    sync.Mutex
	hosts []string
}

func (r *stageRequests) add(host string) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.hosts = append(r.hosts, host)
}

func (r *stageRequests) count(host string) int {
	r.mu.Lock()
	defer r.mu.Unlock()
	count := 0
	for _, item := range r.hosts {
		if item == host {
			count++
		}
	}
	return count
}

type stageTransport struct {
	bodies       map[string][]byte
	requests     *stageRequests
	redirectHost string
}

func (t *stageTransport) RoundTrip(request *http.Request) (*http.Response, error) {
	if err := request.Context().Err(); err != nil {
		return nil, err
	}
	t.requests.add(request.URL.Hostname())
	name := filepath.Base(request.URL.Path)
	if request.URL.Hostname() == "github.com" {
		host := t.redirectHost
		if host == "" {
			host = "release-assets.githubusercontent.com"
		}
		return response(request, http.StatusFound, nil, map[string]string{
			"Location": "https://" + host + "/release/" + name + "?token=bounded",
		}), nil
	}
	body, ok := t.bodies[name]
	if !ok {
		return response(request, http.StatusNotFound, nil, nil), nil
	}
	return response(request, http.StatusOK, body, map[string]string{
		"Content-Length": fmt.Sprint(len(body)),
	}), nil
}

func response(request *http.Request, status int, body []byte, headers map[string]string) *http.Response {
	header := make(http.Header)
	for key, value := range headers {
		header.Set(key, value)
	}
	contentLength := int64(-1)
	if value := header.Get("Content-Length"); value != "" {
		_, _ = fmt.Sscan(value, &contentLength)
	}
	return &http.Response{
		StatusCode:    status,
		Status:        fmt.Sprintf("%d test", status),
		Header:        header,
		Body:          io.NopCloser(bytes.NewReader(body)),
		Request:       request,
		ContentLength: contentLength,
	}
}

func stageFixture(t *testing.T, target Target, packageBytes []byte) (*Client, Candidate, *stageRequests) {
	t.Helper()
	name, err := packageName(target, "1.2.4")
	if err != nil {
		t.Fatal(err)
	}
	digest := sha256.Sum256(packageBytes)
	manifest := []byte(hex.EncodeToString(digest[:]) + "  " + name + "\n")
	requests := &stageRequests{}
	transport := &stageTransport{
		bodies:   map[string][]byte{name: packageBytes, "checksums.txt": manifest},
		requests: requests,
	}
	client := &Client{endpoint: latestReleaseURL, client: &http.Client{Transport: transport}}
	candidate := Candidate{
		Version: "1.2.4",
		Tag:     "v1.2.4",
		Package: Asset{
			Name: name, DownloadURL: "https://github.com/ro-ag/ptrack/releases/download/v1.2.4/" + name,
			SizeBytes: int64(len(packageBytes)),
		},
		Checksums: Asset{
			Name: "checksums.txt", DownloadURL: "https://github.com/ro-ag/ptrack/releases/download/v1.2.4/checksums.txt",
			SizeBytes: int64(len(manifest)),
		},
	}
	return client, candidate, requests
}

func rawRelease(_ *testing.T, _ Target, payload []byte) []byte { return payload }

func tarRelease(t *testing.T, _ Target, payload []byte) []byte {
	t.Helper()
	return customTar(t, []tarItem{
		{name: "./", mode: tar.TypeDir},
		{name: "./ptrack", body: payload, mode: tar.TypeReg},
		{name: "./README.md", body: []byte("readme"), mode: tar.TypeReg},
		{name: "./LICENSE", body: []byte("license"), mode: tar.TypeReg},
	})
}

type tarItem struct {
	name string
	body []byte
	mode byte
}

func customTar(t *testing.T, items []tarItem) []byte {
	t.Helper()
	var buffer bytes.Buffer
	gz := gzip.NewWriter(&buffer)
	archive := tar.NewWriter(gz)
	for _, item := range items {
		header := &tar.Header{Name: item.name, Mode: 0o755, Size: int64(len(item.body)), Typeflag: item.mode}
		if item.mode == tar.TypeDir {
			header.Size = 0
		}
		if err := archive.WriteHeader(header); err != nil {
			t.Fatal(err)
		}
		if len(item.body) > 0 {
			if _, err := archive.Write(item.body); err != nil {
				t.Fatal(err)
			}
		}
	}
	if err := archive.Close(); err != nil {
		t.Fatal(err)
	}
	if err := gz.Close(); err != nil {
		t.Fatal(err)
	}
	return buffer.Bytes()
}

func zipRelease(t *testing.T, _ Target, payload []byte) []byte {
	t.Helper()
	return customZip(t, []zipItem{
		{name: "ptrack.exe", body: payload},
		{name: "README.md", body: []byte("readme")},
		{name: "LICENSE", body: []byte("license")},
	})
}

type zipItem struct {
	name string
	body []byte
}

func customZip(t *testing.T, items []zipItem) []byte {
	t.Helper()
	var buffer bytes.Buffer
	archive := zip.NewWriter(&buffer)
	for _, item := range items {
		header := &zip.FileHeader{Name: item.name, Method: zip.Deflate}
		header.SetMode(0o600)
		writer, err := archive.CreateHeader(header)
		if err != nil {
			t.Fatal(err)
		}
		if _, err := writer.Write(item.body); err != nil {
			t.Fatal(err)
		}
	}
	if err := archive.Close(); err != nil {
		t.Fatal(err)
	}
	return buffer.Bytes()
}

func fakeELF(t *testing.T, arch string) []byte {
	t.Helper()
	data := make([]byte, 64)
	copy(data, []byte{0x7f, 'E', 'L', 'F'})
	data[4] = byte(elfClass64)
	data[5] = byte(elfDataLittleEndian)
	data[6] = 1
	binary.LittleEndian.PutUint16(data[16:18], 2)
	machine := uint16(62)
	if arch == "arm64" {
		machine = 183
	}
	binary.LittleEndian.PutUint16(data[18:20], machine)
	binary.LittleEndian.PutUint32(data[20:24], 1)
	binary.LittleEndian.PutUint16(data[52:54], 64)
	return data
}

const (
	elfClass64          = 2
	elfDataLittleEndian = 1
)

func fakePE(t *testing.T, arch string) []byte {
	t.Helper()
	// A minimal DOS-wrapped PE32+ executable. There are no sections because
	// staging validates format identity and architecture, not execution.
	const peOffset = 0x80
	data := make([]byte, peOffset+4+20+240)
	data[0], data[1] = 'M', 'Z'
	binary.LittleEndian.PutUint32(data[0x3c:0x40], peOffset)
	copy(data[peOffset:peOffset+4], []byte{'P', 'E', 0, 0})
	header := data[peOffset+4:]
	machine := uint16(peMachineAMD64)
	if arch == "arm64" {
		machine = peMachineARM64
	}
	binary.LittleEndian.PutUint16(header[0:2], machine)
	binary.LittleEndian.PutUint16(header[16:18], 240)
	binary.LittleEndian.PutUint16(header[18:20], 0x0002)
	binary.LittleEndian.PutUint16(header[20:22], 0x020b)
	binary.LittleEndian.PutUint32(header[20+108:20+112], 16)
	return data
}

func fakeCOFF() []byte {
	data := make([]byte, 20+240)
	binary.LittleEndian.PutUint16(data[0:2], peMachineAMD64)
	binary.LittleEndian.PutUint16(data[16:18], 240)
	binary.LittleEndian.PutUint16(data[18:20], 0x0002)
	binary.LittleEndian.PutUint16(data[20:22], 0x020b)
	binary.LittleEndian.PutUint32(data[20+108:20+112], 16)
	return data
}

const (
	peMachineAMD64 = 0x8664
	peMachineARM64 = 0xaa64
)

func assertStageRejected(t *testing.T, target Target, packageBytes []byte) {
	t.Helper()
	client, candidate, _ := stageFixture(t, target, packageBytes)
	if _, err := client.Stage(context.Background(), candidate, target, t.TempDir(), nil); !errors.Is(err, ErrInvalidStage) {
		t.Fatalf("error = %v, want ErrInvalidStage", err)
	}
}
