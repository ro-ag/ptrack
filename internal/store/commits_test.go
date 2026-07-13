package store

import (
	"testing"

	"github.com/ro-ag/ptrack/internal/model"
)

func TestCommitAddDedupAndLink(t *testing.T) {
	s := openTemp(t)
	p, _ := s.AddPlan("p")
	tk, _ := s.AddTask(p.ID, "t")

	c1, err := s.AddCommit("abc123def", "#1 implement x", p.ID, tk.ID)
	if err != nil {
		t.Fatal(err)
	}
	// same SHA -> dedup, same id
	c2, _ := s.AddCommit("abc123def", "#1 implement x", p.ID, tk.ID)
	if c1.ID != c2.ID {
		t.Errorf("dedup failed: %d vs %d", c1.ID, c2.ID)
	}
	s.AddCommit("ffff000", "#1 more", p.ID, tk.ID)
	s.AddCommit("eeee111", "unrelated", p.ID, 0)

	byTask, _ := s.CommitsByTask(tk.ID)
	if len(byTask) != 2 {
		t.Errorf("CommitsByTask = %d want 2", len(byTask))
	}
	byPlan, _ := s.CommitsByPlan(p.ID)
	if len(byPlan) != 3 {
		t.Errorf("CommitsByPlan = %d want 3", len(byPlan))
	}
	c, _ := s.Counts()
	if c.Commits != 3 {
		t.Errorf("Counts.Commits = %d want 3", c.Commits)
	}
}

func TestV2DBMigratesToV3(t *testing.T) {
	path := t.TempDir() + "/p.db"
	s, _ := Open(path)
	setFormat(t, s, 2)
	_ = s.Close()
	s2, err := Open(path)
	if err != nil {
		t.Fatalf("reopen v2: %v", err)
	}
	defer s2.Close()
	m, _ := s2.GetMeta()
	if m.FormatVersion != CurrentFormat {
		t.Errorf("format = %d want %d", m.FormatVersion, CurrentFormat)
	}
	if _, err := s2.AddCommit("x", "y", 0, 0); err != nil {
		t.Errorf("commits bucket unusable: %v", err)
	}
	_ = model.Commit{}
}
