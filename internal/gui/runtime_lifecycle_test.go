package gui

import (
	"context"
	"testing"
	"time"
)

func TestOnBeforeCloseWaitsForRuntimeCallAndRejectsNewCalls(t *testing.T) {
	app := newRuntimeLifecycleTestApp(context.Background(), time.Second)
	_, release, ok := app.acquireRuntimeCall()
	if !ok {
		t.Fatal("initial runtime call was rejected")
	}
	result := make(chan bool, 1)
	go func() {
		result <- app.onBeforeClose(context.Background())
	}()
	waitForFrontendClosing(t, app)
	app.lifecycleMu.Lock()
	ctxDuringClose := app.wailsContext
	app.lifecycleMu.Unlock()
	if ctxDuringClose == nil {
		t.Fatal("Wails context was cleared before shutdown")
	}
	if _, _, accepted := app.acquireRuntimeCall(); accepted {
		t.Fatal("runtime call was accepted after close started")
	}
	select {
	case prevent := <-result:
		t.Fatalf("OnBeforeClose returned %t before the in-flight call completed", prevent)
	case <-time.After(20 * time.Millisecond):
	}
	release()
	select {
	case prevent := <-result:
		if prevent {
			t.Fatal("close was prevented after runtime calls drained")
		}
	case <-time.After(time.Second):
		t.Fatal("OnBeforeClose did not finish after the runtime call drained")
	}
}

func TestOnBeforeCloseTimeoutPreventsCloseAndRestoresRuntimeAcceptance(t *testing.T) {
	type contextKey struct{}
	initialCtx := context.WithValue(context.Background(), contextKey{}, "initial")
	app := newRuntimeLifecycleTestApp(initialCtx, 15*time.Millisecond)
	_, releaseInitial, ok := app.acquireRuntimeCall()
	if !ok {
		t.Fatal("initial runtime call was rejected")
	}
	defer releaseInitial()

	if prevent := app.onBeforeClose(context.Background()); !prevent {
		t.Fatal("timed-out runtime drain did not prevent close")
	}
	restoredCtx, releaseRestored, accepted := app.acquireRuntimeCall()
	if !accepted {
		t.Fatal("runtime calls were not restored after close was prevented")
	}
	defer releaseRestored()
	if got := restoredCtx.Value(contextKey{}); got != "initial" {
		t.Fatalf("restored context marker = %v, want initial", got)
	}
}

func TestOnShutdownFencesAndWaitsForRuntimeCall(t *testing.T) {
	app := newRuntimeLifecycleTestApp(context.Background(), time.Second)
	_, release, ok := app.acquireRuntimeCall()
	if !ok {
		t.Fatal("initial runtime call was rejected")
	}
	done := make(chan struct{})
	go func() {
		app.onShutdown(context.Background())
		close(done)
	}()
	waitForFrontendClosing(t, app)
	if _, _, accepted := app.acquireRuntimeCall(); accepted {
		t.Fatal("runtime call was accepted during shutdown")
	}
	select {
	case <-done:
		t.Fatal("shutdown returned before the in-flight runtime call completed")
	case <-time.After(20 * time.Millisecond):
	}
	release()
	select {
	case <-done:
	case <-time.After(time.Second):
		t.Fatal("shutdown did not finish after the runtime call drained")
	}
}

func newRuntimeLifecycleTestApp(ctx context.Context, waitTimeout time.Duration) *App {
	app := newWorkspaceCoordinator(nil, nil)
	app.wailsContext = ctx
	app.runtimeCallTimeout = waitTimeout
	return app
}

func waitForFrontendClosing(t *testing.T, app *App) {
	t.Helper()
	deadline := time.After(time.Second)
	for {
		app.lifecycleMu.Lock()
		closing := app.frontendClosing
		app.lifecycleMu.Unlock()
		if closing {
			return
		}
		select {
		case <-deadline:
			t.Fatal("frontend close fence did not start")
		default:
			time.Sleep(time.Millisecond)
		}
	}
}
