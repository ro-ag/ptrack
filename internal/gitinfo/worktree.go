package gitinfo

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"
)

const maxWorktrees = 64

// WorktreeIdentity is content-free host-observed repository metadata. It does
// not imply permission to read, write, launch, or run Git in the worktree.
type WorktreeIdentity struct {
	Root         string `json:"root"`
	GitDir       string `json:"gitDir"`
	CommonGitDir string `json:"commonGitDir"`
	Branch       string `json:"branch,omitempty"`
	Head         string `json:"head"`
	Linked       bool   `json:"linked"`
}

type ExistingWorktree struct {
	Root   string `json:"root"`
	Branch string `json:"branch,omitempty"`
	Head   string `json:"head"`
}

type WorktreeBounds struct {
	Shown int `json:"shown"`
	Total int `json:"total"`
	More  int `json:"more"`
}

// InspectWorktree validates candidate against the repository containing
// projectRoot using read-only Git commands and canonical filesystem identity.
func (s Service) InspectWorktree(
	ctx context.Context,
	projectRoot string,
	candidate string,
) (WorktreeIdentity, error) {
	canonicalCandidate, err := canonicalExistingPath(candidate)
	if err != nil {
		return WorktreeIdentity{}, fmt.Errorf("canonicalize selected worktree path: %w", err)
	}
	project, err := s.inspectWorktreeIdentity(ctx, projectRoot)
	if err != nil {
		return WorktreeIdentity{}, fmt.Errorf("inspect project repository: %w", err)
	}
	selected, err := s.inspectWorktreeIdentity(ctx, candidate)
	if err != nil {
		return WorktreeIdentity{}, fmt.Errorf("inspect selected worktree: %w", err)
	}
	if project.CommonGitDir != selected.CommonGitDir {
		return WorktreeIdentity{}, errors.New("selected worktree belongs to a different repository")
	}
	if !pathWithinRoot(selected.Root, canonicalCandidate) {
		return WorktreeIdentity{}, errors.New("selected path is outside the inspected worktree")
	}
	runner := s.Runner
	if runner == nil {
		runner = ExecRunner{}
	}
	listedOutput, err := runner.Output(ctx, projectRoot, "worktree", "list", "--porcelain", "-z")
	if err != nil {
		return WorktreeIdentity{}, fmt.Errorf("list repository worktrees: %w", err)
	}
	listed, _, err := parseWorktreeList(listedOutput)
	if err != nil {
		return WorktreeIdentity{}, fmt.Errorf("parse repository worktrees: %w", err)
	}
	member := false
	for _, worktree := range listed {
		if worktree.Root == selected.Root {
			member = true
			break
		}
	}
	if !member {
		return WorktreeIdentity{}, errors.New("selected worktree is not registered with the project repository")
	}
	return selected, nil
}

func pathWithinRoot(root, candidate string) bool {
	relative, err := filepath.Rel(filepath.Clean(root), filepath.Clean(candidate))
	return err == nil && relative != ".." &&
		!strings.HasPrefix(relative, ".."+string(filepath.Separator))
}

func (s Service) inspectWorktreeIdentity(
	ctx context.Context,
	root string,
) (WorktreeIdentity, error) {
	runner := s.Runner
	if runner == nil {
		runner = ExecRunner{}
	}
	output, err := runner.Output(ctx, root,
		"rev-parse", "--path-format=absolute", "--is-inside-work-tree",
		"--show-toplevel", "--absolute-git-dir", "--git-common-dir",
		"--is-bare-repository", "--verify", "HEAD",
	)
	if err != nil {
		return WorktreeIdentity{}, err
	}
	lines := strings.Split(strings.TrimSpace(string(output)), "\n")
	if len(lines) != 6 {
		return WorktreeIdentity{}, errors.New("malformed worktree identity")
	}
	inside, insideErr := strconv.ParseBool(lines[0])
	bare, bareErr := strconv.ParseBool(lines[4])
	if insideErr != nil || bareErr != nil || !inside || bare {
		return WorktreeIdentity{}, errors.New("selected path is not a non-bare worktree")
	}
	canonical := make([]string, 3)
	for index, path := range lines[1:4] {
		canonical[index], err = canonicalExistingPath(path)
		if err != nil {
			return WorktreeIdentity{}, err
		}
	}
	for _, index := range []int{0, 1, 2} {
		info, statErr := os.Stat(canonical[index])
		if statErr != nil || !info.IsDir() {
			return WorktreeIdentity{}, errors.New("worktree identity path is not a directory")
		}
	}
	branchOutput, branchErr := runner.Output(ctx, canonical[0],
		"symbolic-ref", "--quiet", "--short", "HEAD",
	)
	if branchErr != nil && !errors.Is(branchErr, ErrCommandFailed) {
		return WorktreeIdentity{}, branchErr
	}
	branch := strings.TrimSpace(string(branchOutput))
	if strings.ContainsAny(branch, "\x00\r\n") || len(branch) > 512 {
		return WorktreeIdentity{}, errors.New("malformed worktree branch")
	}
	head := strings.TrimSpace(lines[5])
	if !validObjectID(head) {
		return WorktreeIdentity{}, errors.New("malformed worktree HEAD")
	}
	return WorktreeIdentity{
		Root: canonical[0], GitDir: canonical[1], CommonGitDir: canonical[2],
		Branch: branch, Head: strings.ToLower(head),
		Linked: canonical[1] != canonical[2],
	}, nil
}

func canonicalExistingPath(path string) (string, error) {
	absolute, err := filepath.Abs(path)
	if err != nil {
		return "", err
	}
	canonical, err := filepath.EvalSymlinks(absolute)
	if err != nil {
		return "", fmt.Errorf("canonicalize worktree identity: %w", err)
	}
	if _, err := os.Stat(canonical); err != nil {
		return "", fmt.Errorf("stat worktree identity: %w", err)
	}
	return filepath.Clean(canonical), nil
}

func parseWorktreeList(output []byte) ([]ExistingWorktree, WorktreeBounds, error) {
	records := bytes.Split(output, []byte{0, 0})
	worktrees := make([]ExistingWorktree, 0, min(len(records), maxWorktrees))
	total := 0
	for _, record := range records {
		if len(record) == 0 {
			continue
		}
		candidate := ExistingWorktree{}
		valid, skip := true, false
		for _, raw := range bytes.Split(record, []byte{0}) {
			key, value, found := bytes.Cut(raw, []byte{' '})
			if !found {
				if bytes.Equal(raw, []byte("bare")) || bytes.HasPrefix(raw, []byte("prunable")) {
					skip = true
				}
				continue
			}
			switch string(key) {
			case "worktree":
				candidate.Root = string(value)
			case "HEAD":
				candidate.Head = strings.ToLower(string(value))
			case "branch":
				candidate.Branch = strings.TrimPrefix(string(value), "refs/heads/")
			}
		}
		if candidate.Root == "" || !filepath.IsAbs(candidate.Root) ||
			!validObjectID(candidate.Head) || strings.ContainsAny(candidate.Branch, "\x00\r\n") ||
			len(candidate.Branch) > 512 {
			valid = false
		}
		if skip {
			continue
		}
		if !valid {
			return nil, WorktreeBounds{}, errors.New("malformed Git worktree list")
		}
		canonicalRoot, err := canonicalExistingPath(candidate.Root)
		if err != nil {
			continue
		}
		info, err := os.Stat(canonicalRoot)
		if err != nil || !info.IsDir() {
			continue
		}
		candidate.Root = canonicalRoot
		total++
		if len(worktrees) < maxWorktrees {
			worktrees = append(worktrees, candidate)
		}
	}
	return worktrees, WorktreeBounds{
		Shown: len(worktrees), Total: total, More: max(0, total-len(worktrees)),
	}, nil
}

func validObjectID(value string) bool {
	if len(value) != 40 && len(value) != 64 {
		return false
	}
	for _, character := range value {
		if !strings.ContainsRune("0123456789abcdefABCDEF", character) {
			return false
		}
	}
	return true
}
