//go:build darwin

package updater

import (
	"context"
	"errors"
	"os"
	"runtime"
	"testing"
	"time"
)

func TestDarwinApplyVerifiesTrustBeforeOpeningDMG(t *testing.T) {
	t.Parallel()
	stage := darwinStage(t)
	var calls []commandCall
	installer := &Installer{
		currentExecutable: os.Executable,
		run: func(_ context.Context, name string, args ...string) ([]byte, error) {
			calls = append(calls, commandCall{name: name, args: append([]string(nil), args...)})
			return nil, nil
		},
	}
	result, err := installer.Apply(context.Background(), stage)
	if err != nil {
		t.Fatal(err)
	}
	if result.Action != ApplyOpenedInstaller || !result.ManualInstall || result.RestartRequired {
		t.Fatalf("result = %#v", result)
	}
	want := []string{"/usr/bin/hdiutil", "/usr/bin/codesign", "/usr/sbin/spctl", "/usr/bin/open"}
	if len(calls) != len(want) {
		t.Fatalf("calls = %#v", calls)
	}
	for index, name := range want {
		if calls[index].name != name || calls[index].args[len(calls[index].args)-1] != stage.AssetPath {
			t.Fatalf("call %d = %#v", index, calls[index])
		}
	}
}

func TestDarwinApplyStopsBeforeOpenWhenVerificationFails(t *testing.T) {
	t.Parallel()
	stage := darwinStage(t)
	var calls int
	installer := &Installer{
		currentExecutable: os.Executable,
		run: func(_ context.Context, _ string, _ ...string) ([]byte, error) {
			calls++
			if calls == 2 {
				return nil, errors.New("invalid signature")
			}
			return nil, nil
		},
	}
	if _, err := installer.Apply(context.Background(), stage); !errors.Is(err, ErrInstallRefused) {
		t.Fatalf("error = %v, want ErrInstallRefused", err)
	}
	if calls != 2 {
		t.Fatalf("verification continued after failure: %d calls", calls)
	}
}

func TestLiveDarwinDMGTrustContract(t *testing.T) {
	if os.Getenv("PTRACK_LIVE_UPDATE_TEST") != "1" {
		t.Skip("set PTRACK_LIVE_UPDATE_TEST=1 to verify the published DMG trust contract")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
	defer cancel()
	target := Target{GOOS: runtime.GOOS, GOARCH: runtime.GOARCH}
	client := NewClient()
	candidate, err := client.Check(ctx, "0.0.0", target)
	if err != nil {
		t.Fatal(err)
	}
	stage, err := client.Stage(ctx, candidate, target, t.TempDir(), nil)
	if err != nil {
		t.Fatal(err)
	}
	commands := []commandCall{
		{name: "/usr/bin/hdiutil", args: []string{"verify", stage.AssetPath}},
		{name: "/usr/bin/codesign", args: []string{"--verify", "--strict", "--verbose=2", "-R=" + darwinPublisherRequirement, stage.AssetPath}},
		{name: "/usr/sbin/spctl", args: []string{"--assess", "--type", "open", "--context", "context:primary-signature", stage.AssetPath}},
	}
	for _, command := range commands {
		if _, err := runBoundedCommand(ctx, command.name, command.args...); err != nil {
			t.Fatalf("%s: %v", command.name, err)
		}
	}
}

func darwinStage(t *testing.T) StagedUpdate {
	t.Helper()
	target := Target{GOOS: "darwin", GOARCH: runtime.GOARCH}
	client, candidate, _ := stageFixture(t, target, []byte("synthetic-dmg"))
	stage, err := client.Stage(context.Background(), candidate, target, t.TempDir(), nil)
	if err != nil {
		t.Fatal(err)
	}
	return stage
}

type commandCall struct {
	name string
	args []string
}
