package gui

import (
	"context"
	"errors"
	"testing"
	"time"
)

func TestLifecycleGroupWaitContextDoesNotOutliveDeadline(t *testing.T) {
	var group lifecycleGroup
	group.Add(1)
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Millisecond)
	defer cancel()
	if err := group.WaitContext(ctx); !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("WaitContext = %v, want deadline", err)
	}
	group.Done()
	if err := group.WaitContext(context.Background()); err != nil {
		t.Fatalf("WaitContext after Done: %v", err)
	}
}
