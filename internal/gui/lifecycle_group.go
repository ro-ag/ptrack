package gui

import (
	"context"
	"sync"
)

// lifecycleGroup is a WaitGroup with a context-aware wait path that does not
// create a waiter goroutine when shutdown reaches its deadline.
type lifecycleGroup struct {
	mu    sync.Mutex
	count int
	zero  chan struct{}
}

func (g *lifecycleGroup) Add(delta int) {
	g.mu.Lock()
	defer g.mu.Unlock()
	next := g.count + delta
	if next < 0 {
		panic("gui: negative lifecycleGroup count")
	}
	if delta > 0 && g.count == 0 {
		g.zero = make(chan struct{})
	}
	g.count = next
	if g.count == 0 && g.zero != nil {
		close(g.zero)
		g.zero = nil
	}
}

func (g *lifecycleGroup) Done() {
	g.Add(-1)
}

func (g *lifecycleGroup) WaitContext(ctx context.Context) error {
	g.mu.Lock()
	if g.count == 0 {
		g.mu.Unlock()
		return nil
	}
	zero := g.zero
	g.mu.Unlock()
	select {
	case <-zero:
		return nil
	case <-ctx.Done():
		return ctx.Err()
	}
}
