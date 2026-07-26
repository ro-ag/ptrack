package gitinfo

import (
	"context"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"
)

func TestCaptureBuildsBoundedReadOnlyGitSnapshot(t *testing.T) {
	now := time.Date(2026, 7, 26, 12, 0, 0, 0, time.UTC)
	oldEpoch := now.Add(-120 * 24 * time.Hour).Unix()
	recentEpoch := now.Add(-time.Hour).Unix()
	runner := &fakeSnapshotRunner{outputs: map[string][]byte{
		"rev-parse": []byte("true\n/repo\n/repo/.git/worktrees/feature\n/repo/.git\nfalse\n"),
		"status": []byte(
			"# branch.oid abc\x00# branch.head feature\x00" +
				"# branch.upstream origin/feature\x00# branch.ab +2 -1\x00" +
				"1 M. N... 1 1 1 a b staged.go\x00? new.go\x00",
		),
		"config": []byte(
			"remote.origin.url\nhttps://example.test/fetch.git\x00" +
				"remote.origin.pushurl\nssh://example.test/push.git\x00" +
				"remote.backup.url\nhttps://example.test/backup.git\x00",
		),
		"for-each-ref:refs/heads": []byte(
			refRecord("refs/heads/feature", "abc", "origin/feature", recentEpoch, "*", "/repo") +
				refRecord("refs/heads/old", "def", "", oldEpoch, " ", ""),
		),
		"for-each-ref:refs/remotes": []byte(
			refRecord("refs/remotes/origin/feature", "abc", "", recentEpoch, " ", ""),
		),
		"rev-list": []byte("1\t2\n"),
	}}
	runner.outputs["log"] = []byte(
		logRecord("abc", "Ada", "ada@example.test", recentEpoch, "Workspace snapshot", "HEAD -> feature", []string{"internal/gui/app.go", "frontend/src/app.js"}) +
			logRecord("def", "Lin", "lin@example.test", oldEpoch, "Old work", "", []string{"README.md"}),
	)
	service := Service{Runner: runner, Now: func() time.Time { return now }}
	snapshot, err := service.Capture(context.Background(), "/repo")
	if err != nil {
		t.Fatalf("Capture: %v", err)
	}
	if snapshot.State != RepositoryReady || snapshot.Root != "/repo" ||
		!snapshot.LinkedWorktree || snapshot.Bare {
		t.Fatalf("repository state = %#v", snapshot)
	}
	if snapshot.Status.Branch != "feature" || snapshot.Status.Staged != 1 ||
		snapshot.Status.Untracked != 1 || snapshot.Status.Ahead != 2 ||
		snapshot.Status.Behind != 1 {
		t.Fatalf("status = %#v", snapshot.Status)
	}
	if len(snapshot.Remotes) != 2 ||
		snapshot.Remotes[0].Name != "backup" ||
		snapshot.Remotes[1].PushURLs[0] != "ssh://example.test/push.git" {
		t.Fatalf("remotes = %#v", snapshot.Remotes)
	}
	if len(snapshot.LocalBranches) != 2 || !snapshot.LocalBranches[1].Stale ||
		snapshot.LocalBranches[0].WorktreePath != "/repo" {
		t.Fatalf("local branches = %#v", snapshot.LocalBranches)
	}
	if len(snapshot.RecentCommits) != 2 ||
		snapshot.RecentCommits[0].AuthorName != "Ada" ||
		snapshot.RecentCommits[0].FilesChanged != 2 ||
		!reflect.DeepEqual(snapshot.RecentCommits[0].ChangedAreas, []ChangedArea{
			{Name: "frontend", Files: 1},
			{Name: "internal", Files: 1},
		}) {
		t.Fatalf("commits = %#v", snapshot.RecentCommits)
	}
	if snapshot.Divergence == nil || snapshot.Divergence.Ahead != 2 ||
		snapshot.Divergence.Behind != 1 || len(snapshot.UnpushedCommits) != 2 {
		t.Fatalf("divergence/unpushed = %#v / %#v", snapshot.Divergence, snapshot.UnpushedCommits)
	}
	if calls := runner.Commands(); len(calls) > maxGitCommands {
		t.Fatalf("commands = %d exceeds %d: %#v", len(calls), maxGitCommands, calls)
	}
	for _, call := range runner.Commands() {
		if isMutatingGitSubcommand(call[0]) {
			t.Fatalf("mutating Git command used: %#v", call)
		}
	}
}

func TestCaptureReportsNonRepositoryWithoutError(t *testing.T) {
	runner := &fakeSnapshotRunner{errors: map[string]error{
		"rev-parse": ErrCommandFailed,
	}}
	snapshot, err := (Service{Runner: runner}).Capture(context.Background(), "/not-repo")
	if err != nil {
		t.Fatalf("Capture: %v", err)
	}
	if snapshot.State != RepositoryNotFound {
		t.Fatalf("state = %q want notRepository", snapshot.State)
	}
}

func TestCapturePropagatesRepositoryIdentityFailureWhenGitMarkerExists(t *testing.T) {
	root := t.TempDir()
	if err := os.Mkdir(filepath.Join(root, ".git"), 0o700); err != nil {
		t.Fatal(err)
	}
	runner := &fakeSnapshotRunner{errors: map[string]error{
		"rev-parse": ErrCommandFailed,
	}}
	_, err := (Service{Runner: runner}).Capture(context.Background(), root)
	if !errors.Is(err, ErrCommandFailed) {
		t.Fatalf("Capture error = %v, want repository failure", err)
	}
}

func TestCapturePropagatesCancellationAndResourceLimits(t *testing.T) {
	for _, test := range []struct {
		name string
		err  error
	}{
		{name: "cancel", err: context.Canceled},
		{name: "timeout", err: ErrCommandTimeout},
		{name: "output", err: ErrOutputLimit},
	} {
		t.Run(test.name, func(t *testing.T) {
			runner := &fakeSnapshotRunner{errors: map[string]error{"rev-parse": test.err}}
			_, err := (Service{Runner: runner}).Capture(context.Background(), "/repo")
			if !errors.Is(err, test.err) {
				t.Fatalf("Capture error = %v want %v", err, test.err)
			}
		})
	}
}

func TestParseRefsAndLogsRejectMalformedRecords(t *testing.T) {
	if _, _, err := parseRefs([]byte("too\x00few\x00fields\x00"), time.Now()); err == nil {
		t.Fatal("parseRefs accepted malformed output")
	}
	if _, err := parseLog([]byte("\x1eonly-one-field\npath\n"), 10); err == nil {
		t.Fatal("parseLog accepted malformed output")
	}
}

func TestCaptureAgainstRealRepository(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("git is unavailable")
	}
	root := t.TempDir()
	runGitForTest(t, root, "init", "-q")
	runGitForTest(t, root, "config", "user.name", "P Track")
	runGitForTest(t, root, "config", "user.email", "ptrack@example.test")
	if err := os.WriteFile(filepath.Join(root, "tracked.txt"), []byte("one\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	runGitForTest(t, root, "add", "tracked.txt")
	runGitForTest(t, root, "commit", "-q", "-m", "initial")
	if err := os.WriteFile(filepath.Join(root, "untracked.txt"), []byte("two\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	snapshot, err := (Service{}).Capture(context.Background(), root)
	if err != nil {
		t.Fatalf("Capture real repository: %v", err)
	}
	canonicalRoot, err := filepath.EvalSymlinks(root)
	if err != nil {
		t.Fatal(err)
	}
	if snapshot.State != RepositoryReady || snapshot.Root != canonicalRoot ||
		snapshot.Status.Untracked != 1 || len(snapshot.RecentCommits) != 1 ||
		snapshot.RecentCommits[0].Subject != "initial" {
		t.Fatalf("real snapshot = %#v", snapshot)
	}
}

func runGitForTest(t *testing.T, root string, args ...string) {
	t.Helper()
	commandArgs := append([]string{"-C", root}, args...)
	command := exec.Command("git", commandArgs...)
	command.Env = gitEnvironment(os.Environ())
	if output, err := command.CombinedOutput(); err != nil {
		t.Fatalf("git %v: %v\n%s", args, err, output)
	}
}

type fakeSnapshotRunner struct {
	mu      sync.Mutex
	outputs map[string][]byte
	errors  map[string]error
	calls   [][]string
}

func (r *fakeSnapshotRunner) Output(
	_ context.Context,
	_ string,
	args ...string,
) ([]byte, error) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.calls = append(r.calls, append([]string(nil), args...))
	key := args[0]
	if key == "for-each-ref" {
		key += ":" + args[len(args)-1]
	}
	if err := r.errors[key]; err != nil {
		return nil, err
	}
	return append([]byte(nil), r.outputs[key]...), nil
}

func (r *fakeSnapshotRunner) Commands() [][]string {
	r.mu.Lock()
	defer r.mu.Unlock()
	return append([][]string(nil), r.calls...)
}

func refRecord(ref, oid, upstream string, epoch int64, head, worktree string) string {
	return strings.Join([]string{
		ref, oid, upstream, strconv.FormatInt(epoch, 10),
		head, worktree, "",
	}, "\x00") + "\n"
}

func logRecord(
	sha, author, email string,
	epoch int64,
	subject, refs string,
	paths []string,
) string {
	return "\x1e" + strings.Join([]string{
		sha, author, email, strconv.FormatInt(epoch, 10),
		subject, refs,
	}, "\x1f") + "\n" + strings.Join(paths, "\n") + "\n"
}
