package gitinfo

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
)

type fakeWorktreeRunner struct {
	outputs map[string][]byte
	errors  map[string]error
}

func (r fakeWorktreeRunner) Output(
	_ context.Context,
	root string,
	args ...string,
) ([]byte, error) {
	key := filepath.Clean(root) + "|" + args[0]
	if err := r.errors[key]; err != nil {
		return nil, err
	}
	output, exists := r.outputs[key]
	if !exists {
		return nil, errors.New("unexpected Git command")
	}
	return output, nil
}

func TestInspectWorktreeRequiresCanonicalSharedRepositoryIdentity(t *testing.T) {
	base, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	project := filepath.Join(base, "project")
	projectGit := filepath.Join(project, ".git")
	sibling := filepath.Join(base, "sibling")
	siblingGit := filepath.Join(projectGit, "worktrees", "sibling")
	otherCommon := filepath.Join(base, "other.git")
	for _, path := range []string{projectGit, sibling, siblingGit, otherCommon} {
		if err := os.MkdirAll(path, 0o700); err != nil {
			t.Fatal(err)
		}
	}
	link := filepath.Join(base, "sibling-link")
	if err := os.Symlink(sibling, link); err != nil {
		t.Fatal(err)
	}
	sha := strings.Repeat("a", 40)
	identity := func(root, gitDir, commonDir string) []byte {
		return []byte("true\n" + root + "\n" + gitDir + "\n" + commonDir +
			"\nfalse\n" + sha + "\n")
	}
	runner := fakeWorktreeRunner{outputs: map[string][]byte{
		project + "|rev-parse":    identity(project, projectGit, projectGit),
		project + "|symbolic-ref": []byte("main\n"),
		project + "|worktree": []byte(
			"worktree " + project + "\x00HEAD " + sha + "\x00branch refs/heads/main\x00\x00" +
				"worktree " + sibling + "\x00HEAD " + sha + "\x00branch refs/heads/feature\x00\x00",
		),
		link + "|rev-parse":       identity(sibling, siblingGit, projectGit),
		sibling + "|symbolic-ref": []byte("feature\n"),
	}}
	service := Service{Runner: runner}
	got, err := service.InspectWorktree(context.Background(), project, link)
	if err != nil {
		t.Fatal(err)
	}
	if got.Root != sibling || got.CommonGitDir != projectGit || !got.Linked ||
		got.Branch != "feature" || got.Head != sha {
		t.Fatalf("identity = %#v", got)
	}

	runner.outputs[link+"|rev-parse"] = identity(sibling, siblingGit, otherCommon)
	if _, err := (Service{Runner: runner}).InspectWorktree(
		context.Background(), project, link,
	); err == nil {
		t.Fatal("different common Git directory was accepted")
	}
	runner.outputs[link+"|rev-parse"] = identity(sibling, siblingGit, projectGit)
	runner.outputs[project+"|worktree"] = []byte(
		"worktree " + project + "\x00HEAD " + sha + "\x00branch refs/heads/main\x00\x00",
	)
	if _, err := (Service{Runner: runner}).InspectWorktree(
		context.Background(), project, link,
	); err == nil {
		t.Fatal("unlisted copied worktree identity was accepted")
	}
}

func TestParseWorktreeListIsBoundedAndContentFree(t *testing.T) {
	base, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	first := filepath.Join(base, "first")
	second := filepath.Join(base, "second")
	for _, path := range []string{first, second} {
		if err := os.Mkdir(path, 0o700); err != nil {
			t.Fatal(err)
		}
	}
	firstSHA := strings.Repeat("a", 40)
	secondSHA := strings.Repeat("b", 40)
	output := []byte(
		"worktree " + first + "\x00HEAD " + firstSHA + "\x00branch refs/heads/main\x00\x00" +
			"worktree " + second + "\x00HEAD " + secondSHA + "\x00detached\x00\x00",
	)
	worktrees, bounds, err := parseWorktreeList(output)
	if err != nil {
		t.Fatal(err)
	}
	want := []ExistingWorktree{
		{Root: first, Branch: "main", Head: firstSHA},
		{Root: second, Head: secondSHA},
	}
	if !reflect.DeepEqual(worktrees, want) || bounds.Total != 2 || bounds.More != 0 {
		t.Fatalf("worktrees = %#v bounds=%#v", worktrees, bounds)
	}
}
