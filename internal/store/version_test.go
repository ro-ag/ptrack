package store

import (
	"errors"
	"path/filepath"
	"testing"

	"github.com/ro-ag/ptrack/internal/model"
	bolt "go.etcd.io/bbolt"
)

// setFormat rewrites the stored meta's FormatVersion, simulating a database
// written by a different ptrack build.
func setFormat(t *testing.T, s *Store, v uint) {
	t.Helper()
	err := s.db.Update(func(tx *bolt.Tx) error {
		mb := tx.Bucket(bucketMeta)
		var m model.Meta
		if err := gobDecode(mb.Get(keyMeta), &m); err != nil {
			return err
		}
		m.FormatVersion = v
		return putGob(mb, keyMeta, m)
	})
	if err != nil {
		t.Fatalf("setFormat: %v", err)
	}
}

func TestFreshDBStampsCurrentFormat(t *testing.T) {
	s := openTemp(t)
	m, _ := s.GetMeta()
	if m.FormatVersion != CurrentFormat {
		t.Errorf("FormatVersion = %d want %d", m.FormatVersion, CurrentFormat)
	}
	if m.LastWriteVersion != WriterVersion {
		t.Errorf("LastWriteVersion = %q want %q", m.LastWriteVersion, WriterVersion)
	}
}

func TestAdoptPreVersioningDB(t *testing.T) {
	path := filepath.Join(t.TempDir(), "ptrack.db")
	s, err := Open(path)
	if err != nil {
		t.Fatal(err)
	}
	setFormat(t, s, 0) // pretend it's a v0.1.0 DB with no version
	_ = s.Close()

	s2, err := Open(path)
	if err != nil {
		t.Fatalf("reopen: %v", err)
	}
	defer s2.Close()
	m, _ := s2.GetMeta()
	if m.FormatVersion != CurrentFormat {
		t.Errorf("adopted FormatVersion = %d want %d", m.FormatVersion, CurrentFormat)
	}
}

func TestRejectNewerFormat(t *testing.T) {
	path := filepath.Join(t.TempDir(), "ptrack.db")
	s, err := Open(path)
	if err != nil {
		t.Fatal(err)
	}
	setFormat(t, s, CurrentFormat+1)
	_ = s.Close()

	_, err = Open(path)
	var tooNew ErrFormatTooNew
	if !errors.As(err, &tooNew) {
		t.Fatalf("want ErrFormatTooNew, got %v", err)
	}
	if tooNew.Found != CurrentFormat+1 || tooNew.Supported != CurrentFormat {
		t.Errorf("ErrFormatTooNew = %+v", tooNew)
	}
}

func TestCounts(t *testing.T) {
	s := openTemp(t)
	p1, _ := s.AddPlan("a")
	p2, _ := s.AddPlan("b")
	s.SetPlanStatus(p2.ID, model.PlanDone)
	t1, _ := s.AddTask(p1.ID, "t1")
	t2, _ := s.AddTask(p1.ID, "t2")
	t3, _ := s.AddTask(p1.ID, "t3")
	s.SetTaskStatus(t1.ID, model.TaskDone)
	s.SetTaskStatus(t2.ID, model.TaskBlocked)
	_ = t3 // stays todo
	s.AddNote(model.TargetProject, 0, "n1")
	s.AddNote(model.TargetPlan, p1.ID, "n2")

	c, err := s.Counts()
	if err != nil {
		t.Fatal(err)
	}
	want := model.Counts{Plans: 2, PlansDone: 1, Tasks: 3, TasksDone: 1, TasksBlocked: 1, TasksOpen: 2, Notes: 2}
	if c != want {
		t.Errorf("counts = %+v want %+v", c, want)
	}
}

func TestNotesByTarget(t *testing.T) {
	s := openTemp(t)
	p, _ := s.AddPlan("p")
	tk, _ := s.AddTask(p.ID, "t")
	s.AddNote(model.TargetProject, 0, "proj")
	s.AddNote(model.TargetPlan, p.ID, "planning")
	s.AddNote(model.TargetTask, tk.ID, "tasking")

	pn, _ := s.NotesByPlan(p.ID)
	if len(pn) != 1 || pn[0].Body != "planning" {
		t.Errorf("NotesByPlan = %+v", pn)
	}
	tn, _ := s.NotesByTask(tk.ID)
	if len(tn) != 1 || tn[0].Body != "tasking" {
		t.Errorf("NotesByTask = %+v", tn)
	}
}
