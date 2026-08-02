package gui

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestWatchWorkspaceDataDebouncesChangeBursts(t *testing.T) {
	dbPath := filepath.Join(t.TempDir(), "ptrack.db")
	if err := os.WriteFile(dbPath, []byte("initial"), 0o600); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	emits := make(chan struct{}, 8)
	done := make(chan struct{})
	go func() {
		defer close(done)
		watchWorkspaceData(ctx, dbPath, 10*time.Millisecond, 50*time.Millisecond, func() {
			emits <- struct{}{}
		})
	}()

	// Baseline: an unchanged file emits nothing.
	select {
	case <-emits:
		t.Fatal("emit before any change")
	case <-time.After(120 * time.Millisecond):
	}

	// A burst of writes inside the debounce window coalesces to one emit.
	for index := range 5 {
		content := strings.Repeat("x", 100+index*50)
		if err := os.WriteFile(dbPath, []byte(content), 0o600); err != nil {
			t.Fatal(err)
		}
		time.Sleep(15 * time.Millisecond)
	}
	select {
	case <-emits:
	case <-time.After(time.Second):
		t.Fatal("no emit after change burst")
	}
	select {
	case <-emits:
		t.Fatal("burst produced more than one emit")
	case <-time.After(200 * time.Millisecond):
	}

	// A write after a quiet period emits again.
	if err := os.WriteFile(dbPath, []byte("later write"), 0o600); err != nil {
		t.Fatal(err)
	}
	select {
	case <-emits:
	case <-time.After(time.Second):
		t.Fatal("no emit after quiet-period change")
	}

	cancel()
	select {
	case <-done:
	case <-time.After(time.Second):
		t.Fatal("watcher did not stop on cancel")
	}
}

func TestWatchWorkspaceDataDetectsRemoval(t *testing.T) {
	dbPath := filepath.Join(t.TempDir(), "ptrack.db")
	if err := os.WriteFile(dbPath, []byte("initial"), 0o600); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	emits := make(chan struct{}, 1)
	done := make(chan struct{})
	go func() {
		defer close(done)
		watchWorkspaceData(ctx, dbPath, 10*time.Millisecond, 20*time.Millisecond, func() {
			emits <- struct{}{}
		})
	}()
	// Let the watcher take its baseline fingerprint before the removal,
	// otherwise a slow runner may only ever observe the file as missing.
	time.Sleep(100 * time.Millisecond)
	if err := os.Remove(dbPath); err != nil {
		t.Fatal(err)
	}
	select {
	case <-emits:
	case <-time.After(time.Second):
		t.Fatal("no emit after database removal")
	}
	cancel()
	<-done
}
