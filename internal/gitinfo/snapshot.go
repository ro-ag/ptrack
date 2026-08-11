package gitinfo

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"time"
)

const (
	maxGitCommands       = 9
	maxAggregateGitBytes = 12 * 1024 * 1024
	maxRemotes           = 16
	maxLocalBranches     = 100
	maxRemoteBranches    = 150
	maxRecentCommits     = 40
	maxUnpushedCommits   = 40
	maxChangedPaths      = 500
	staleBranchAge       = 90 * 24 * time.Hour
)

var ErrAggregateLimit = errors.New("git snapshot aggregate limit exceeded")

type RepositoryState string

const (
	RepositoryReady    RepositoryState = "ready"
	RepositoryNotFound RepositoryState = "notRepository"
)

type Remote struct {
	Name      string   `json:"name"`
	FetchURLs []string `json:"fetchUrls"`
	PushURLs  []string `json:"pushUrls"`
}

type Branch struct {
	Name         string `json:"name"`
	Ref          string `json:"ref"`
	OID          string `json:"oid"`
	Upstream     string `json:"upstream"`
	LastCommitAt string `json:"lastCommitAt"`
	Current      bool   `json:"current"`
	Remote       bool   `json:"remote"`
	WorktreePath string `json:"worktreePath"`
	Stale        bool   `json:"stale"`
}

type ChangedArea struct {
	Name  string `json:"name"`
	Files int    `json:"files"`
}

type Commit struct {
	SHA          string        `json:"sha"`
	AuthorName   string        `json:"authorName"`
	AuthorEmail  string        `json:"authorEmail"`
	Date         string        `json:"date"`
	Subject      string        `json:"subject"`
	Refs         []string      `json:"refs"`
	FilesChanged int           `json:"filesChanged"`
	ChangedAreas []ChangedArea `json:"changedAreas"`
}

type Divergence struct {
	Upstream string `json:"upstream"`
	Ahead    int    `json:"ahead"`
	Behind   int    `json:"behind"`
}

type Snapshot struct {
	State                    RepositoryState    `json:"state"`
	Root                     string             `json:"root"`
	GitDir                   string             `json:"gitDir"`
	CommonGitDir             string             `json:"commonGitDir"`
	Bare                     bool               `json:"bare"`
	LinkedWorktree           bool               `json:"linkedWorktree"`
	Status                   Status             `json:"status"`
	Remotes                  []Remote           `json:"remotes"`
	LocalBranches            []Branch           `json:"localBranches"`
	RemoteBranches           []Branch           `json:"remoteBranches"`
	RecentCommits            []Commit           `json:"recentCommits"`
	UnpushedCommits          []Commit           `json:"unpushedCommits"`
	RecentCommitsTruncated   bool               `json:"recentCommitsTruncated"`
	UnpushedCommitsTruncated bool               `json:"unpushedCommitsTruncated"`
	Worktrees                []ExistingWorktree `json:"worktrees"`
	WorktreeBounds           WorktreeBounds     `json:"worktreeBounds"`
	WorktreesIncomplete      bool               `json:"worktreesIncomplete"`
	Divergence               *Divergence        `json:"divergence,omitempty"`
	StaleBranchPolicy        string             `json:"staleBranchPolicy"`
}

type Service struct {
	Runner Runner
	Now    func() time.Time
}

type captureBudget struct {
	commands int
	bytes    int
}

func (s Service) Capture(ctx context.Context, root string) (Snapshot, error) {
	runner := s.Runner
	if runner == nil {
		runner = ExecRunner{}
	}
	now := time.Now
	if s.Now != nil {
		now = s.Now
	}
	budget := &captureBudget{}
	run := func(args ...string) ([]byte, error) {
		budget.commands++
		if budget.commands > maxGitCommands {
			return nil, ErrAggregateLimit
		}
		output, err := runner.Output(ctx, root, args...)
		if err != nil {
			return nil, err
		}
		budget.bytes += len(output)
		if budget.bytes > maxAggregateGitBytes {
			return nil, ErrAggregateLimit
		}
		return output, nil
	}

	identity, err := run(
		"rev-parse",
		"--path-format=absolute",
		"--is-inside-work-tree",
		"--show-toplevel",
		"--absolute-git-dir",
		"--git-common-dir",
		"--is-bare-repository",
	)
	if err != nil {
		if errors.Is(err, ErrCommandFailed) {
			hasMarker, markerErr := hasGitWorktreeMarker(root)
			if markerErr != nil {
				return Snapshot{}, errors.Join(err, markerErr)
			}
			if !hasMarker {
				return Snapshot{State: RepositoryNotFound}, nil
			}
		}
		return Snapshot{}, err
	}
	snapshot, err := parseRepositoryIdentity(identity)
	if err != nil {
		return Snapshot{}, err
	}
	worktreeOutput, worktreeErr := run("worktree", "list", "--porcelain", "-z")
	if worktreeErr == nil {
		snapshot.Worktrees, snapshot.WorktreeBounds, err = parseWorktreeList(worktreeOutput)
		if err != nil {
			return Snapshot{}, err
		}
		snapshot.WorktreesIncomplete = snapshot.WorktreeBounds.More > 0
	} else if errors.Is(worktreeErr, ErrCommandFailed) {
		snapshot.WorktreesIncomplete = true
	} else {
		return Snapshot{}, worktreeErr
	}

	statusOutput, err := run(
		"status",
		"--porcelain=v2",
		"--branch",
		"-z",
		"--ignored=matching",
	)
	if err != nil {
		return Snapshot{}, err
	}
	snapshot.Status, err = parsePorcelainV2Status(statusOutput)
	if err != nil {
		return Snapshot{}, err
	}

	remoteOutput, err := run(
		"config",
		"--null",
		"--get-regexp",
		`^remote\..*\.(url|pushurl)$`,
	)
	if err == nil {
		snapshot.Remotes, err = parseRemotes(remoteOutput)
		if err != nil {
			return Snapshot{}, err
		}
	} else if !errors.Is(err, ErrCommandFailed) {
		return Snapshot{}, err
	}

	localRefOutput, err := run(
		"for-each-ref",
		"--count="+strconv.Itoa(maxLocalBranches),
		"--sort=-committerdate",
		"--format=%(refname)%00%(objectname)%00%(upstream:short)%00%(committerdate:unix)%00%(HEAD)%00%(worktreepath)%00",
		"refs/heads",
	)
	if err != nil {
		return Snapshot{}, err
	}
	localBranches, unexpectedRemotes, err := parseRefs(localRefOutput, now())
	if err != nil {
		return Snapshot{}, err
	}
	if len(unexpectedRemotes) != 0 {
		return Snapshot{}, errors.New("local Git ref query returned remote refs")
	}
	remoteRefOutput, err := run(
		"for-each-ref",
		"--count="+strconv.Itoa(maxRemoteBranches),
		"--sort=-committerdate",
		"--format=%(refname)%00%(objectname)%00%(upstream:short)%00%(committerdate:unix)%00%(HEAD)%00%(worktreepath)%00",
		"refs/remotes",
	)
	if err != nil {
		return Snapshot{}, err
	}
	unexpectedLocals, remoteBranches, err := parseRefs(remoteRefOutput, now())
	if err != nil {
		return Snapshot{}, err
	}
	if len(unexpectedLocals) != 0 {
		return Snapshot{}, errors.New("remote Git ref query returned local refs")
	}
	snapshot.LocalBranches = localBranches
	snapshot.RemoteBranches = remoteBranches

	logOutput, err := run(
		"log",
		"-n", strconv.Itoa(maxRecentCommits+1),
		"--date=unix",
		"--format=%x1e%H%x1f%an%x1f%ae%x1f%at%x1f%s%x1f%D",
		"--name-only",
	)
	if err == nil {
		snapshot.RecentCommits, err = parseLog(logOutput, maxRecentCommits+1)
		if err != nil {
			return Snapshot{}, err
		}
		if len(snapshot.RecentCommits) > maxRecentCommits {
			snapshot.RecentCommits = snapshot.RecentCommits[:maxRecentCommits]
			snapshot.RecentCommitsTruncated = true
		}
	} else if !errors.Is(err, ErrCommandFailed) {
		return Snapshot{}, err
	}

	if snapshot.Status.Upstream != "" {
		divergenceOutput, divergenceErr := run(
			"rev-list",
			"--left-right",
			"--count",
			snapshot.Status.Upstream+"...HEAD",
		)
		if divergenceErr != nil {
			return Snapshot{}, divergenceErr
		}
		snapshot.Divergence, err = parseDivergence(
			snapshot.Status.Upstream,
			divergenceOutput,
		)
		if err != nil {
			return Snapshot{}, err
		}
		unpushedOutput, unpushedErr := run(
			"log",
			"-n", strconv.Itoa(maxUnpushedCommits+1),
			"--date=unix",
			"--format=%x1e%H%x1f%an%x1f%ae%x1f%at%x1f%s%x1f%D",
			"--name-only",
			snapshot.Status.Upstream+"..HEAD",
		)
		if unpushedErr == nil {
			snapshot.UnpushedCommits, err = parseLog(unpushedOutput, maxUnpushedCommits+1)
			if err != nil {
				return Snapshot{}, err
			}
			if len(snapshot.UnpushedCommits) > maxUnpushedCommits {
				snapshot.UnpushedCommits = snapshot.UnpushedCommits[:maxUnpushedCommits]
				snapshot.UnpushedCommitsTruncated = true
			}
		} else if !errors.Is(unpushedErr, ErrCommandFailed) {
			return Snapshot{}, unpushedErr
		}
	}
	snapshot.StaleBranchPolicy = "non-current local branch tip older than 90 days; not proof that deletion is safe"
	return snapshot, nil
}

func hasGitWorktreeMarker(root string) (bool, error) {
	current, err := filepath.Abs(root)
	if err != nil {
		return false, err
	}
	for {
		_, statErr := os.Lstat(filepath.Join(current, ".git"))
		switch {
		case statErr == nil:
			return true, nil
		case !errors.Is(statErr, os.ErrNotExist):
			return false, statErr
		}
		parent := filepath.Dir(current)
		if parent == current {
			return false, nil
		}
		current = parent
	}
}

func parseRepositoryIdentity(output []byte) (Snapshot, error) {
	lines := strings.Split(strings.TrimSpace(string(output)), "\n")
	if len(lines) != 5 {
		return Snapshot{}, errors.New("malformed git repository identity")
	}
	inside, err := strconv.ParseBool(lines[0])
	if err != nil || !inside {
		return Snapshot{}, errors.New("git root is not a worktree")
	}
	bare, err := strconv.ParseBool(lines[4])
	if err != nil {
		return Snapshot{}, fmt.Errorf("parse bare repository state: %w", err)
	}
	gitDir := filepath.Clean(lines[2])
	commonDir := filepath.Clean(lines[3])
	return Snapshot{
		State:          RepositoryReady,
		Root:           filepath.Clean(lines[1]),
		GitDir:         gitDir,
		CommonGitDir:   commonDir,
		Bare:           bare,
		LinkedWorktree: gitDir != commonDir,
	}, nil
}

func parseRemotes(output []byte) ([]Remote, error) {
	type remoteURLs struct {
		fetch []string
		push  []string
	}
	byName := make(map[string]*remoteURLs)
	for _, raw := range bytes.Split(output, []byte{0}) {
		if len(raw) == 0 {
			continue
		}
		key, value, found := strings.Cut(string(raw), "\n")
		if !found || !strings.HasPrefix(key, "remote.") {
			return nil, errors.New("malformed Git remote config")
		}
		var name string
		var push bool
		switch {
		case strings.HasSuffix(key, ".pushurl"):
			name = strings.TrimSuffix(strings.TrimPrefix(key, "remote."), ".pushurl")
			push = true
		case strings.HasSuffix(key, ".url"):
			name = strings.TrimSuffix(strings.TrimPrefix(key, "remote."), ".url")
		default:
			return nil, errors.New("unexpected Git remote config key")
		}
		if name == "" {
			return nil, errors.New("empty Git remote name")
		}
		entry := byName[name]
		if entry == nil {
			entry = &remoteURLs{}
			byName[name] = entry
		}
		if push {
			entry.push = append(entry.push, value)
		} else {
			entry.fetch = append(entry.fetch, value)
		}
	}
	names := make([]string, 0, len(byName))
	for name := range byName {
		names = append(names, name)
	}
	sort.Strings(names)
	if len(names) > maxRemotes {
		names = names[:maxRemotes]
	}
	remotes := make([]Remote, 0, len(names))
	for _, name := range names {
		urls := byName[name]
		push := urls.push
		if len(push) == 0 {
			push = append([]string(nil), urls.fetch...)
		}
		remotes = append(remotes, Remote{
			Name:      name,
			FetchURLs: append([]string(nil), urls.fetch...),
			PushURLs:  append([]string(nil), push...),
		})
	}
	return remotes, nil
}

func parseRefs(output []byte, now time.Time) ([]Branch, []Branch, error) {
	fields := bytes.Split(output, []byte{0})
	local := make([]Branch, 0)
	remote := make([]Branch, 0)
	for index := 0; index < len(fields); {
		ref := strings.TrimPrefix(string(fields[index]), "\n")
		if ref == "" {
			index++
			continue
		}
		if index+5 >= len(fields) {
			return nil, nil, errors.New("malformed Git ref output")
		}
		epoch, err := strconv.ParseInt(string(fields[index+3]), 10, 64)
		if err != nil {
			return nil, nil, fmt.Errorf("parse ref commit date: %w", err)
		}
		commitTime := time.Unix(epoch, 0)
		branch := Branch{
			Ref:          ref,
			OID:          string(fields[index+1]),
			Upstream:     string(fields[index+2]),
			LastCommitAt: commitTime.UTC().Format(time.RFC3339),
			Current:      string(fields[index+4]) == "*",
			WorktreePath: string(fields[index+5]),
		}
		switch {
		case strings.HasPrefix(ref, "refs/heads/"):
			branch.Name = strings.TrimPrefix(ref, "refs/heads/")
			branch.Stale = !branch.Current && now.Sub(commitTime) > staleBranchAge
			if len(local) < maxLocalBranches {
				local = append(local, branch)
			}
		case strings.HasPrefix(ref, "refs/remotes/"):
			branch.Name = strings.TrimPrefix(ref, "refs/remotes/")
			branch.Remote = true
			if !strings.HasSuffix(branch.Name, "/HEAD") &&
				len(remote) < maxRemoteBranches {
				remote = append(remote, branch)
			}
		default:
			return nil, nil, errors.New("unexpected Git ref namespace")
		}
		index += 6
	}
	return local, remote, nil
}

func parseLog(output []byte, limit int) ([]Commit, error) {
	records := bytes.Split(output, []byte{0x1e})
	commits := make([]Commit, 0, min(limit, len(records)))
	for _, raw := range records {
		raw = bytes.TrimSpace(raw)
		if len(raw) == 0 {
			continue
		}
		header, paths, _ := bytes.Cut(raw, []byte{'\n'})
		fields := bytes.Split(header, []byte{0x1f})
		if len(fields) != 6 {
			return nil, errors.New("malformed Git log record")
		}
		epoch, err := strconv.ParseInt(string(fields[3]), 10, 64)
		if err != nil {
			return nil, fmt.Errorf("parse commit date: %w", err)
		}
		refs := make([]string, 0)
		for _, ref := range strings.Split(string(fields[5]), ",") {
			if ref = strings.TrimSpace(ref); ref != "" {
				refs = append(refs, ref)
			}
		}
		pathLines := bytes.Split(bytes.TrimSpace(paths), []byte{'\n'})
		if len(pathLines) == 1 && len(pathLines[0]) == 0 {
			pathLines = nil
		}
		if len(pathLines) > maxChangedPaths {
			pathLines = pathLines[:maxChangedPaths]
		}
		areas := changedAreas(pathLines)
		commits = append(commits, Commit{
			SHA:          string(fields[0]),
			AuthorName:   string(fields[1]),
			AuthorEmail:  string(fields[2]),
			Date:         time.Unix(epoch, 0).UTC().Format(time.RFC3339),
			Subject:      string(fields[4]),
			Refs:         refs,
			FilesChanged: len(pathLines),
			ChangedAreas: areas,
		})
		if len(commits) >= limit {
			break
		}
	}
	return commits, nil
}

func changedAreas(paths [][]byte) []ChangedArea {
	counts := make(map[string]int)
	for _, raw := range paths {
		path := strings.TrimSpace(string(raw))
		if path == "" {
			continue
		}
		area, _, found := strings.Cut(filepath.ToSlash(path), "/")
		if !found {
			area = "(root)"
		}
		counts[area]++
	}
	areas := make([]ChangedArea, 0, len(counts))
	for name, files := range counts {
		areas = append(areas, ChangedArea{Name: name, Files: files})
	}
	sort.Slice(areas, func(i, j int) bool {
		if areas[i].Files == areas[j].Files {
			return areas[i].Name < areas[j].Name
		}
		return areas[i].Files > areas[j].Files
	})
	if len(areas) > 6 {
		areas = areas[:6]
	}
	return areas
}

func parseDivergence(upstream string, output []byte) (*Divergence, error) {
	fields := strings.Fields(string(output))
	if len(fields) != 2 {
		return nil, errors.New("malformed Git divergence output")
	}
	behind, err := strconv.Atoi(fields[0])
	if err != nil {
		return nil, fmt.Errorf("parse behind count: %w", err)
	}
	ahead, err := strconv.Atoi(fields[1])
	if err != nil {
		return nil, fmt.Errorf("parse ahead count: %w", err)
	}
	return &Divergence{Upstream: upstream, Ahead: ahead, Behind: behind}, nil
}

func isMutatingGitSubcommand(command string) bool {
	switch command {
	case "add", "branch", "checkout", "clean", "commit", "fetch", "merge",
		"pull", "push", "rebase", "reset", "restore", "switch", "tag":
		return true
	default:
		return false
	}
}
