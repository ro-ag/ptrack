package updater

import (
	"archive/tar"
	"archive/zip"
	"bufio"
	"compress/gzip"
	"context"
	"crypto/sha256"
	"debug/elf"
	"debug/pe"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"time"
)

const (
	maxArchiveEntryBytes = 128 << 20
	maxArchiveTotalBytes = 160 << 20
	maxChecksumLines     = 256
	maxChecksumLineBytes = 512
	maxAssetDownloadTime = 10 * time.Minute
)

var ErrInvalidStage = errors.New("invalid staged update")

// StageKind identifies the verified payload preserved for installation.
type StageKind string

const (
	StageDarwinDMG   StageKind = "darwin-dmg"
	StageLinuxBinary StageKind = "linux-binary"
	StageWindowsZIP  StageKind = "windows-zip"
)

// Progress reports bounded download progress. Asset is either "checksums" or
// "package"; paths and URLs are deliberately omitted.
type Progress struct {
	Asset      string
	Downloaded int64
	Total      int64
}

// ProgressFunc receives staging progress on the caller's goroutine.
type ProgressFunc func(Progress)

// StagedUpdate contains only private local paths and verified release facts.
type StagedUpdate struct {
	Root             string
	AssetPath        string
	PayloadPath      string
	StatePath        string
	Version          string
	AssetName        string
	GOOS             string
	GOARCH           string
	SHA256           string
	SizeBytes        int64
	PayloadSHA256    string
	PayloadSizeBytes int64
	Kind             StageKind
}

type stageRecord struct {
	Version          string    `json:"version"`
	AssetName        string    `json:"asset_name"`
	GOOS             string    `json:"goos"`
	GOARCH           string    `json:"goarch"`
	SHA256           string    `json:"sha256"`
	SizeBytes        int64     `json:"size_bytes"`
	PayloadSHA256    string    `json:"payload_sha256"`
	PayloadSizeBytes int64     `json:"payload_size_bytes"`
	Kind             StageKind `json:"kind"`
}

// Stage downloads the exact candidate package into a new private directory,
// verifies checksums.txt, validates the archive layout and machine type, and
// persists a bounded recovery record. Any failure removes the partial stage.
func (c *Client) Stage(
	ctx context.Context,
	candidate Candidate,
	target Target,
	baseDir string,
	progress ProgressFunc,
) (stage StagedUpdate, err error) {
	expectedName, err := packageName(target, candidate.Version)
	if err != nil {
		return StagedUpdate{}, err
	}
	if candidate.Tag != "v"+candidate.Version || candidate.Package.Name != expectedName ||
		candidate.Checksums.Name != "checksums.txt" {
		return StagedUpdate{}, fmt.Errorf("%w: candidate identity mismatch", ErrInvalidStage)
	}
	if _, err := parseVersion(candidate.Version, true); err != nil {
		return StagedUpdate{}, fmt.Errorf("%w: invalid candidate version", ErrInvalidStage)
	}
	if err := validateAssetURL(candidate.Package.DownloadURL, candidate.Tag, candidate.Package.Name); err != nil {
		return StagedUpdate{}, fmt.Errorf("%w: %v", ErrInvalidStage, err)
	}
	if err := validateAssetURL(candidate.Checksums.DownloadURL, candidate.Tag, candidate.Checksums.Name); err != nil {
		return StagedUpdate{}, fmt.Errorf("%w: %v", ErrInvalidStage, err)
	}
	if candidate.Package.SizeBytes <= 0 || candidate.Package.SizeBytes > maxAssetBytes ||
		candidate.Checksums.SizeBytes <= 0 || candidate.Checksums.SizeBytes > maxManifestBytes {
		return StagedUpdate{}, fmt.Errorf("%w: invalid candidate sizes", ErrInvalidStage)
	}

	root, err := makeStageRoot(baseDir)
	if err != nil {
		return StagedUpdate{}, err
	}
	keep := false
	defer func() {
		if !keep {
			_ = os.RemoveAll(root)
		}
	}()

	manifestPath := filepath.Join(root, "checksums.txt")
	if _, _, err := c.download(ctx, candidate.Checksums, manifestPath, "checksums", progress); err != nil {
		return StagedUpdate{}, err
	}
	wantedDigest, err := checksumFor(manifestPath, candidate.Package.Name)
	if err != nil {
		return StagedUpdate{}, err
	}

	assetPath := filepath.Join(root, candidate.Package.Name)
	digest, size, err := c.download(ctx, candidate.Package, assetPath, "package", progress)
	if err != nil {
		return StagedUpdate{}, err
	}
	if digest != wantedDigest {
		return StagedUpdate{}, fmt.Errorf("%w: checksum mismatch for %q", ErrInvalidStage, candidate.Package.Name)
	}

	stage = StagedUpdate{
		Root:      root,
		AssetPath: assetPath,
		Version:   candidate.Version,
		AssetName: candidate.Package.Name,
		GOOS:      target.GOOS,
		GOARCH:    target.GOARCH,
		SHA256:    digest,
		SizeBytes: size,
		StatePath: filepath.Join(root, "state.json"),
	}
	switch target.GOOS {
	case "darwin":
		stage.Kind = StageDarwinDMG
		stage.PayloadPath = assetPath
	case "linux":
		stage.Kind = StageLinuxBinary
		stage.PayloadPath = filepath.Join(root, "ptrack")
		if err := extractTarPayload(ctx, assetPath, stage.PayloadPath); err != nil {
			return StagedUpdate{}, err
		}
	case "windows":
		stage.Kind = StageWindowsZIP
		stage.PayloadPath = filepath.Join(root, "ptrack.exe")
		if err := extractZipPayload(ctx, assetPath, stage.PayloadPath); err != nil {
			return StagedUpdate{}, err
		}
	}
	if err := validatePayloadMachine(ctx, stage); err != nil {
		return StagedUpdate{}, err
	}
	payloadLimit := int64(maxArchiveEntryBytes)
	if stage.Kind == StageDarwinDMG {
		payloadLimit = maxAssetBytes
	}
	stage.PayloadSHA256, stage.PayloadSizeBytes, err = hashRegularFile(ctx, stage.PayloadPath, payloadLimit)
	if err != nil {
		return StagedUpdate{}, fmt.Errorf("%w: hash staged payload: %v", ErrInvalidStage, err)
	}
	if err := writeStageRecord(stage); err != nil {
		return StagedUpdate{}, err
	}
	keep = true
	return stage, nil
}

// ValidateStage revalidates the durable record, package digest, size, paths,
// archive payload, and machine type immediately before installation.
func ValidateStage(stage StagedUpdate) error {
	return ValidateStageContext(context.Background(), stage)
}

// ValidateStageContext is ValidateStage with cancellation for hashing and
// executable parsing before an install attempt.
func ValidateStageContext(ctx context.Context, stage StagedUpdate) error {
	if err := ctx.Err(); err != nil {
		return err
	}
	if stage.Root == "" || !filepath.IsAbs(stage.Root) {
		return fmt.Errorf("%w: stage root is not absolute", ErrInvalidStage)
	}
	for _, path := range []string{stage.AssetPath, stage.PayloadPath, stage.StatePath} {
		if !pathWithin(stage.Root, path) {
			return fmt.Errorf("%w: path escapes stage root", ErrInvalidStage)
		}
	}
	if err := validatePrivatePath(stage.Root, true); err != nil {
		return fmt.Errorf("%w: unsafe stage root", ErrInvalidStage)
	}
	version, err := parseVersion(stage.Version, true)
	payloadLimit := int64(maxArchiveEntryBytes)
	if stage.Kind == StageDarwinDMG {
		payloadLimit = maxAssetBytes
	}
	if err != nil || version.String() != stage.Version || stage.SizeBytes <= 0 || stage.SizeBytes > maxAssetBytes ||
		stage.PayloadSizeBytes <= 0 || stage.PayloadSizeBytes > payloadLimit ||
		!validDigest(stage.SHA256) || !validDigest(stage.PayloadSHA256) {
		return fmt.Errorf("%w: invalid stage identity", ErrInvalidStage)
	}
	expectedName, err := packageName(Target{GOOS: stage.GOOS, GOARCH: stage.GOARCH}, stage.Version)
	if err != nil || stage.AssetName != expectedName || stage.AssetPath != filepath.Join(stage.Root, expectedName) ||
		stage.StatePath != filepath.Join(stage.Root, "state.json") {
		return fmt.Errorf("%w: stage path identity mismatch", ErrInvalidStage)
	}
	expectedPayload := stage.AssetPath
	if stage.Kind == StageLinuxBinary {
		expectedPayload = filepath.Join(stage.Root, "ptrack")
	} else if stage.Kind == StageWindowsZIP {
		expectedPayload = filepath.Join(stage.Root, "ptrack.exe")
	}
	if stage.PayloadPath != expectedPayload {
		return fmt.Errorf("%w: payload path identity mismatch", ErrInvalidStage)
	}
	for _, path := range []string{stage.AssetPath, stage.PayloadPath, stage.StatePath} {
		if err := validatePrivatePath(path, false); err != nil {
			return fmt.Errorf("%w: unsafe staged file", ErrInvalidStage)
		}
	}
	recordBytes, err := readPrivateFile(ctx, stage.StatePath, 4096)
	if err != nil {
		return fmt.Errorf("%w: read state record", ErrInvalidStage)
	}
	var record stageRecord
	if err := decodeStageRecord(recordBytes, &record); err != nil || record != stage.record() {
		return fmt.Errorf("%w: state record mismatch", ErrInvalidStage)
	}
	digest, size, err := hashRegularFile(ctx, stage.AssetPath, stage.SizeBytes)
	if err != nil || digest != stage.SHA256 || size != stage.SizeBytes {
		return fmt.Errorf("%w: package changed after staging", ErrInvalidStage)
	}
	if err := validatePayloadMachine(ctx, stage); err != nil {
		return err
	}
	payloadDigest, payloadSize, err := hashRegularFile(ctx, stage.PayloadPath, payloadLimit)
	if err != nil || payloadDigest != stage.PayloadSHA256 || payloadSize != stage.PayloadSizeBytes {
		return fmt.Errorf("%w: payload changed after staging", ErrInvalidStage)
	}
	return nil
}

// LoadStage reconstructs and validates a durable stage after process restart.
func LoadStage(root string) (StagedUpdate, error) {
	return LoadStageContext(context.Background(), root)
}

// LoadStageContext reloads and validates a durable stage with cancellation.
func LoadStageContext(ctx context.Context, root string) (StagedUpdate, error) {
	if err := ctx.Err(); err != nil {
		return StagedUpdate{}, err
	}
	if root == "" || !filepath.IsAbs(root) {
		return StagedUpdate{}, fmt.Errorf("%w: stage root is not absolute", ErrInvalidStage)
	}
	statePath := filepath.Join(root, "state.json")
	if err := validatePrivatePath(root, true); err != nil {
		return StagedUpdate{}, fmt.Errorf("%w: unsafe stage root", ErrInvalidStage)
	}
	if err := validatePrivatePath(statePath, false); err != nil {
		return StagedUpdate{}, fmt.Errorf("%w: unsafe stage record", ErrInvalidStage)
	}
	data, err := readPrivateFile(ctx, statePath, 4096)
	if err != nil {
		return StagedUpdate{}, fmt.Errorf("%w: read stage record", ErrInvalidStage)
	}
	var record stageRecord
	if err := decodeStageRecord(data, &record); err != nil {
		return StagedUpdate{}, fmt.Errorf("%w: decode stage record", ErrInvalidStage)
	}
	name, err := packageName(Target{GOOS: record.GOOS, GOARCH: record.GOARCH}, record.Version)
	if err != nil || name != record.AssetName {
		return StagedUpdate{}, fmt.Errorf("%w: stage record identity mismatch", ErrInvalidStage)
	}
	stage := StagedUpdate{
		Root: root, AssetPath: filepath.Join(root, name), StatePath: statePath,
		Version: record.Version, AssetName: record.AssetName, GOOS: record.GOOS, GOARCH: record.GOARCH,
		SHA256: record.SHA256, SizeBytes: record.SizeBytes,
		PayloadSHA256: record.PayloadSHA256, PayloadSizeBytes: record.PayloadSizeBytes, Kind: record.Kind,
	}
	switch stage.Kind {
	case StageDarwinDMG:
		stage.PayloadPath = stage.AssetPath
	case StageLinuxBinary:
		stage.PayloadPath = filepath.Join(root, "ptrack")
	case StageWindowsZIP:
		stage.PayloadPath = filepath.Join(root, "ptrack.exe")
	default:
		return StagedUpdate{}, fmt.Errorf("%w: unknown stage kind", ErrInvalidStage)
	}
	if err := ValidateStageContext(ctx, stage); err != nil {
		return StagedUpdate{}, err
	}
	return stage, nil
}

func makeStageRoot(baseDir string) (string, error) {
	if baseDir == "" || !filepath.IsAbs(baseDir) {
		return "", fmt.Errorf("%w: update directory is not absolute", ErrInvalidStage)
	}
	if err := preparePrivateDir(baseDir); err != nil {
		return "", fmt.Errorf("%w: protect update directory: %v", ErrInvalidStage, err)
	}
	root, err := os.MkdirTemp(baseDir, ".stage-")
	if err != nil {
		return "", fmt.Errorf("create update stage: %w", err)
	}
	if err := securePrivatePath(root, true); err != nil {
		_ = os.RemoveAll(root)
		return "", fmt.Errorf("protect update stage: %w", err)
	}
	return root, nil
}

func (c *Client) download(
	ctx context.Context,
	asset Asset,
	destination string,
	progressName string,
	progress ProgressFunc,
) (digest string, size int64, err error) {
	if c == nil || c.client == nil {
		return "", 0, fmt.Errorf("%w: release client is not configured", ErrInvalidStage)
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, asset.DownloadURL, nil)
	if err != nil {
		return "", 0, fmt.Errorf("build asset request: %w", err)
	}
	req.Header.Set("Accept", "application/octet-stream")
	req.Header.Set("User-Agent", userAgent)

	httpClient := *c.client
	httpClient.Timeout = maxAssetDownloadTime
	httpClient.CheckRedirect = func(req *http.Request, via []*http.Request) error {
		if len(via) >= 3 {
			return errors.New("too many asset redirects")
		}
		req.Header.Del("Authorization")
		req.Header.Del("Cookie")
		if err := validateDownloadURL(req.URL, via[0].URL); err != nil {
			return err
		}
		return nil
	}
	resp, err := httpClient.Do(req)
	if err != nil {
		if ctx.Err() != nil {
			return "", 0, fmt.Errorf("download %s: %w", progressName, ctx.Err())
		}
		if errors.Is(err, context.DeadlineExceeded) {
			return "", 0, fmt.Errorf("download %s: %w", progressName, context.DeadlineExceeded)
		}
		if errors.Is(err, ErrInvalidStage) {
			return "", 0, fmt.Errorf("download %s: %w", progressName, ErrInvalidStage)
		}
		return "", 0, fmt.Errorf("download %s failed", progressName)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return "", 0, fmt.Errorf("download %s: unexpected HTTP status %d", progressName, resp.StatusCode)
	}
	if err := validateDownloadURL(resp.Request.URL, req.URL); err != nil {
		return "", 0, fmt.Errorf("download %s: %w", progressName, err)
	}
	if resp.ContentLength >= 0 && resp.ContentLength != asset.SizeBytes {
		return "", 0, fmt.Errorf("%w: %s size differs from release metadata", ErrInvalidStage, progressName)
	}

	file, err := os.OpenFile(destination, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o600)
	if err != nil {
		return "", 0, fmt.Errorf("create staged %s: %w", progressName, err)
	}
	keep := false
	defer func() {
		_ = file.Close()
		if !keep {
			_ = os.Remove(destination)
		}
	}()
	hash := sha256.New()
	reader := io.LimitReader(resp.Body, asset.SizeBytes+1)
	written, err := io.Copy(io.MultiWriter(file, hash), &progressReader{
		reader: reader,
		asset:  progressName,
		total:  asset.SizeBytes,
		notify: progress,
	})
	if err != nil {
		return "", 0, fmt.Errorf("write staged %s: %w", progressName, err)
	}
	if written != asset.SizeBytes {
		return "", 0, fmt.Errorf("%w: %s size mismatch", ErrInvalidStage, progressName)
	}
	if err := file.Sync(); err != nil {
		return "", 0, fmt.Errorf("sync staged %s: %w", progressName, err)
	}
	if err := file.Close(); err != nil {
		return "", 0, fmt.Errorf("close staged %s: %w", progressName, err)
	}
	if err := securePrivatePath(destination, false); err != nil {
		return "", 0, fmt.Errorf("protect staged %s: %w", progressName, err)
	}
	keep = true
	return hex.EncodeToString(hash.Sum(nil)), written, nil
}

type progressReader struct {
	reader     io.Reader
	asset      string
	total      int64
	downloaded int64
	notify     ProgressFunc
}

func (r *progressReader) Read(buffer []byte) (int, error) {
	n, err := r.reader.Read(buffer)
	if n > 0 {
		r.downloaded += int64(n)
		if r.notify != nil {
			r.notify(Progress{Asset: r.asset, Downloaded: r.downloaded, Total: r.total})
		}
	}
	return n, err
}

func validateDownloadURL(candidate, initial *url.URL) error {
	if candidate == nil || initial == nil || candidate.Scheme != "https" || candidate.User != nil ||
		candidate.Fragment != "" || strings.Contains(candidate.RawFragment, "#") {
		return fmt.Errorf("%w: unsafe asset redirect", ErrInvalidStage)
	}
	if candidate.Host == "github.com" {
		if candidate.String() != initial.String() {
			return fmt.Errorf("%w: unexpected GitHub asset redirect", ErrInvalidStage)
		}
		return nil
	}
	if candidate.Port() != "" {
		return fmt.Errorf("%w: asset redirect uses a port", ErrInvalidStage)
	}
	switch candidate.Hostname() {
	case "release-assets.githubusercontent.com", "objects.githubusercontent.com":
		if candidate.Path == "" || candidate.Path == "/" {
			return fmt.Errorf("%w: empty asset redirect path", ErrInvalidStage)
		}
		return nil
	default:
		return fmt.Errorf("%w: asset redirect left GitHub", ErrInvalidStage)
	}
}

func checksumFor(path, wantedName string) (string, error) {
	file, err := os.Open(path)
	if err != nil {
		return "", fmt.Errorf("read checksum manifest: %w", err)
	}
	defer file.Close()
	scanner := bufio.NewScanner(file)
	scanner.Buffer(make([]byte, 1024), maxChecksumLineBytes)
	found := ""
	lines := 0
	for scanner.Scan() {
		lines++
		if lines > maxChecksumLines {
			return "", fmt.Errorf("%w: too many checksum entries", ErrInvalidStage)
		}
		line := strings.TrimSuffix(scanner.Text(), "\r")
		if len(line) < 66 || line[64] != ' ' {
			return "", fmt.Errorf("%w: malformed checksum entry", ErrInvalidStage)
		}
		digest := line[:64]
		if decoded, err := hex.DecodeString(digest); err != nil || len(decoded) != sha256.Size {
			return "", fmt.Errorf("%w: malformed checksum digest", ErrInvalidStage)
		}
		name := strings.TrimSpace(line[64:])
		if name == "" || strings.ContainsAny(name, "/\\\t") || filepath.Base(name) != name {
			return "", fmt.Errorf("%w: unsafe checksum filename", ErrInvalidStage)
		}
		if name == wantedName {
			if found != "" {
				return "", fmt.Errorf("%w: duplicate checksum for %q", ErrInvalidStage, wantedName)
			}
			found = strings.ToLower(digest)
		}
	}
	if err := scanner.Err(); err != nil {
		return "", fmt.Errorf("read checksum manifest: %w", err)
	}
	if found == "" {
		return "", fmt.Errorf("%w: checksum missing for %q", ErrInvalidStage, wantedName)
	}
	return found, nil
}

func extractTarPayload(ctx context.Context, assetPath, payloadPath string) error {
	file, err := openPrivateRegular(assetPath)
	if err != nil {
		return fmt.Errorf("open update archive: %w", err)
	}
	defer file.Close()
	gz, err := gzip.NewReader(file)
	if err != nil {
		return fmt.Errorf("%w: open gzip archive: %v", ErrInvalidStage, err)
	}
	defer gz.Close()
	archive := tar.NewReader(io.LimitReader(gz, maxArchiveTotalBytes+1))
	wanted := map[string]bool{"ptrack": false, "README.md": false, "LICENSE": false}
	var total int64
	for {
		if err := ctx.Err(); err != nil {
			return err
		}
		header, err := archive.Next()
		if errors.Is(err, io.EOF) {
			break
		}
		if err != nil {
			return fmt.Errorf("%w: read tar archive: %v", ErrInvalidStage, err)
		}
		name := strings.TrimPrefix(header.Name, "./")
		if name == "." || name == "" {
			if header.Typeflag != tar.TypeDir {
				return fmt.Errorf("%w: invalid tar root entry", ErrInvalidStage)
			}
			continue
		}
		if _, ok := wanted[name]; !ok || wanted[name] || strings.ContainsAny(name, "/\\") {
			return fmt.Errorf("%w: unexpected or duplicate tar entry %q", ErrInvalidStage, header.Name)
		}
		if header.Typeflag != tar.TypeReg && header.Typeflag != tar.TypeRegA {
			return fmt.Errorf("%w: tar entry %q is not regular", ErrInvalidStage, header.Name)
		}
		if header.Size < 0 || header.Size > maxArchiveEntryBytes || total+header.Size > maxArchiveTotalBytes {
			return fmt.Errorf("%w: tar archive is oversized", ErrInvalidStage)
		}
		total += header.Size
		wanted[name] = true
		if name == "ptrack" {
			if err := copyPayload(ctx, payloadPath, archive, header.Size); err != nil {
				return err
			}
		} else if _, err := io.CopyN(io.Discard, &contextReader{ctx: ctx, reader: archive}, header.Size); err != nil {
			return fmt.Errorf("%w: truncated tar entry %q", ErrInvalidStage, header.Name)
		}
	}
	for name, present := range wanted {
		if !present {
			return fmt.Errorf("%w: missing tar entry %q", ErrInvalidStage, name)
		}
	}
	return nil
}

func extractZipPayload(ctx context.Context, assetPath, payloadPath string) error {
	asset, err := openPrivateRegular(assetPath)
	if err != nil {
		return fmt.Errorf("%w: open zip archive: %v", ErrInvalidStage, err)
	}
	defer asset.Close()
	info, err := asset.Stat()
	if err != nil {
		return fmt.Errorf("%w: stat zip archive: %v", ErrInvalidStage, err)
	}
	archive, err := zip.NewReader(asset, info.Size())
	if err != nil {
		return fmt.Errorf("%w: open zip archive: %v", ErrInvalidStage, err)
	}
	wanted := map[string]bool{"ptrack.exe": false, "README.md": false, "LICENSE": false}
	var total uint64
	for _, file := range archive.File {
		if err := ctx.Err(); err != nil {
			return err
		}
		name := strings.TrimPrefix(file.Name, "./")
		if _, ok := wanted[name]; !ok || wanted[name] || strings.ContainsAny(name, "/\\") {
			return fmt.Errorf("%w: unexpected or duplicate zip entry %q", ErrInvalidStage, file.Name)
		}
		if file.Flags&1 != 0 || !file.Mode().IsRegular() || file.UncompressedSize64 > maxArchiveEntryBytes ||
			total+file.UncompressedSize64 > maxArchiveTotalBytes {
			return fmt.Errorf("%w: unsafe zip entry %q", ErrInvalidStage, file.Name)
		}
		total += file.UncompressedSize64
		wanted[name] = true
		reader, err := file.Open()
		if err != nil {
			return fmt.Errorf("%w: open zip entry %q", ErrInvalidStage, file.Name)
		}
		if name == "ptrack.exe" {
			err = copyPayload(ctx, payloadPath, reader, int64(file.UncompressedSize64))
		} else {
			_, err = io.CopyN(io.Discard, &contextReader{ctx: ctx, reader: reader}, int64(file.UncompressedSize64))
		}
		closeErr := reader.Close()
		if err != nil || closeErr != nil {
			return fmt.Errorf("%w: read zip entry %q", ErrInvalidStage, file.Name)
		}
	}
	for name, present := range wanted {
		if !present {
			return fmt.Errorf("%w: missing zip entry %q", ErrInvalidStage, name)
		}
	}
	return nil
}

func copyPayload(ctx context.Context, path string, reader io.Reader, size int64) error {
	file, err := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o700)
	if err != nil {
		return fmt.Errorf("create staged payload: %w", err)
	}
	keep := false
	defer func() {
		_ = file.Close()
		if !keep {
			_ = os.Remove(path)
		}
	}()
	written, err := io.CopyN(file, &contextReader{ctx: ctx, reader: reader}, size)
	if err != nil || written != size {
		return fmt.Errorf("%w: truncated archive payload", ErrInvalidStage)
	}
	if err := file.Sync(); err != nil {
		return fmt.Errorf("sync staged payload: %w", err)
	}
	if err := file.Close(); err != nil {
		return fmt.Errorf("close staged payload: %w", err)
	}
	if err := securePrivatePath(path, false); err != nil {
		return fmt.Errorf("protect staged payload: %w", err)
	}
	keep = true
	return nil
}

func validatePayloadMachine(ctx context.Context, stage StagedUpdate) error {
	if err := ctx.Err(); err != nil {
		return err
	}
	switch stage.Kind {
	case StageDarwinDMG:
		if stage.GOOS != "darwin" || stage.PayloadPath != stage.AssetPath {
			return fmt.Errorf("%w: invalid macOS stage", ErrInvalidStage)
		}
		return nil
	case StageLinuxBinary:
		if stage.GOOS != "linux" {
			return fmt.Errorf("%w: platform mismatch", ErrInvalidStage)
		}
		payload, err := openPrivateRegular(stage.PayloadPath)
		if err != nil {
			return fmt.Errorf("%w: staged Linux payload is not ELF", ErrInvalidStage)
		}
		defer payload.Close()
		file, err := elf.NewFile(payload)
		if err != nil {
			return fmt.Errorf("%w: staged Linux payload is not ELF", ErrInvalidStage)
		}
		defer file.Close()
		if file.Class != elf.ELFCLASS64 || file.Data != elf.ELFDATA2LSB ||
			(file.Type != elf.ET_EXEC && file.Type != elf.ET_DYN) {
			return fmt.Errorf("%w: staged Linux payload is not a 64-bit executable", ErrInvalidStage)
		}
		want := elf.EM_X86_64
		if stage.GOARCH == "arm64" {
			want = elf.EM_AARCH64
		}
		if file.Machine != want {
			return fmt.Errorf("%w: staged Linux machine mismatch", ErrInvalidStage)
		}
		return nil
	case StageWindowsZIP:
		if stage.GOOS != "windows" {
			return fmt.Errorf("%w: platform mismatch", ErrInvalidStage)
		}
		payload, err := openPrivateRegular(stage.PayloadPath)
		if err != nil {
			return fmt.Errorf("%w: staged Windows payload is not PE", ErrInvalidStage)
		}
		defer payload.Close()
		if err := validatePESignature(payload); err != nil {
			return fmt.Errorf("%w: staged Windows payload is not PE", ErrInvalidStage)
		}
		file, err := pe.NewFile(payload)
		if err != nil {
			return fmt.Errorf("%w: staged Windows payload is not PE", ErrInvalidStage)
		}
		defer file.Close()
		if file.OptionalHeader == nil || file.Characteristics&pe.IMAGE_FILE_EXECUTABLE_IMAGE == 0 {
			return fmt.Errorf("%w: staged Windows payload is not an executable image", ErrInvalidStage)
		}
		want := uint16(pe.IMAGE_FILE_MACHINE_AMD64)
		if stage.GOARCH == "arm64" {
			want = pe.IMAGE_FILE_MACHINE_ARM64
		}
		if file.Machine != want {
			return fmt.Errorf("%w: staged Windows machine mismatch", ErrInvalidStage)
		}
		return nil
	default:
		return fmt.Errorf("%w: unknown stage kind", ErrInvalidStage)
	}
}

func writeStageRecord(stage StagedUpdate) error {
	data, err := json.Marshal(stage.record())
	if err != nil {
		return fmt.Errorf("encode stage record: %w", err)
	}
	data = append(data, '\n')
	file, err := os.OpenFile(stage.StatePath, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o600)
	if err != nil {
		return fmt.Errorf("create stage record: %w", err)
	}
	if _, err := file.Write(data); err != nil {
		_ = file.Close()
		return fmt.Errorf("write stage record: %w", err)
	}
	if err := file.Sync(); err != nil {
		_ = file.Close()
		return fmt.Errorf("sync stage record: %w", err)
	}
	if err := file.Close(); err != nil {
		return fmt.Errorf("close stage record: %w", err)
	}
	if err := securePrivatePath(stage.StatePath, false); err != nil {
		return fmt.Errorf("protect stage record: %w", err)
	}
	return nil
}

func (s StagedUpdate) record() stageRecord {
	return stageRecord{
		Version: s.Version, AssetName: s.AssetName, GOOS: s.GOOS, GOARCH: s.GOARCH,
		SHA256: s.SHA256, SizeBytes: s.SizeBytes,
		PayloadSHA256: s.PayloadSHA256, PayloadSizeBytes: s.PayloadSizeBytes, Kind: s.Kind,
	}
}

func decodeStageRecord(data []byte, record *stageRecord) error {
	decoder := json.NewDecoder(strings.NewReader(string(data)))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(record); err != nil {
		return err
	}
	if err := decoder.Decode(&struct{}{}); !errors.Is(err, io.EOF) {
		return errors.New("stage record has trailing data")
	}
	return nil
}

func validDigest(value string) bool {
	decoded, err := hex.DecodeString(value)
	return err == nil && len(value) == sha256.Size*2 && len(decoded) == sha256.Size && value == strings.ToLower(value)
}

func hashRegularFile(ctx context.Context, path string, limit int64) (string, int64, error) {
	file, err := openPrivateRegular(path)
	if err != nil {
		return "", 0, err
	}
	defer file.Close()
	hash := sha256.New()
	size, err := io.Copy(hash, io.LimitReader(&contextReader{ctx: ctx, reader: file}, limit+1))
	if err != nil {
		return "", 0, err
	}
	return hex.EncodeToString(hash.Sum(nil)), size, nil
}

func readPrivateFile(ctx context.Context, path string, limit int64) ([]byte, error) {
	file, err := openPrivateRegular(path)
	if err != nil {
		return nil, err
	}
	defer file.Close()
	data, err := io.ReadAll(io.LimitReader(&contextReader{ctx: ctx, reader: file}, limit+1))
	if err != nil {
		return nil, err
	}
	if int64(len(data)) > limit {
		return nil, errors.New("private file exceeds limit")
	}
	return data, nil
}

type contextReader struct {
	ctx    context.Context
	reader io.Reader
}

func (r *contextReader) Read(buffer []byte) (int, error) {
	if err := r.ctx.Err(); err != nil {
		return 0, err
	}
	return r.reader.Read(buffer)
}

func validatePESignature(file *os.File) error {
	var dos [64]byte
	if _, err := file.ReadAt(dos[:], 0); err != nil || dos[0] != 'M' || dos[1] != 'Z' {
		return errors.New("missing DOS header")
	}
	offset := int64(binary.LittleEndian.Uint32(dos[0x3c:]))
	info, err := file.Stat()
	if err != nil || offset < int64(len(dos)) || offset > info.Size()-4 {
		return errors.New("invalid PE signature offset")
	}
	var signature [4]byte
	if _, err := file.ReadAt(signature[:], offset); err != nil || signature != [4]byte{'P', 'E', 0, 0} {
		return errors.New("invalid PE signature")
	}
	return nil
}

func pathWithin(root, path string) bool {
	if path == "" || !filepath.IsAbs(path) {
		return false
	}
	relative, err := filepath.Rel(root, path)
	return err == nil && relative != "." && relative != ".." && !strings.HasPrefix(relative, ".."+string(filepath.Separator))
}
