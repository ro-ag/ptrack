package capability

import (
	"sync"
	"testing"
)

func TestProcessOutputUsesOneAggregateBudget(t *testing.T) {
	budget := newBoundedProcessBudget(8)
	stdout := newBoundedProcessBuffer(budget)
	stderr := newBoundedProcessBuffer(budget)
	var writers sync.WaitGroup
	writers.Add(2)
	go func() {
		defer writers.Done()
		_, _ = stdout.Write([]byte("stdout"))
	}()
	go func() {
		defer writers.Done()
		_, _ = stderr.Write([]byte("stderr"))
	}()
	writers.Wait()
	if got := len(stdout.String()) + len(stderr.String()); got != 8 {
		t.Fatalf("aggregate output = %d bytes, want 8", got)
	}
	if !budget.Truncated() {
		t.Fatal("aggregate overflow was not reported")
	}
}
