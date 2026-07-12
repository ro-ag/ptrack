// Package store persists ptrack data in bbolt. Struct values are gob-encoded.
// It exposes a project store (plans/tasks/notes for one project) and a global
// store (config, project registry, backups).
package store

import (
	"errors"
	"fmt"
	"time"

	"github.com/ro-ag/ptrack/internal/model"
	bolt "go.etcd.io/bbolt"
)

// ErrNotFound is returned when a requested plan, task, or note id does not exist.
var ErrNotFound = errors.New("not found")

// CurrentFormat is the on-disk schema version this build writes and understands.
const CurrentFormat uint = 1

// WriterVersion is the ptrack semver recorded on writes for diagnostics. main
// sets it from the resolved CLI version; it defaults to "dev".
var WriterVersion = "dev"

// ErrFormatTooNew is returned when a database was written by a newer ptrack whose
// on-disk format this build does not understand. The database is left untouched.
type ErrFormatTooNew struct {
	Found     uint
	Supported uint
}

func (e ErrFormatTooNew) Error() string {
	return fmt.Sprintf("database format v%d is newer than this ptrack (supports v%d) — upgrade ptrack", e.Found, e.Supported)
}

var (
	bucketMeta  = []byte("meta")
	bucketPlans = []byte("plans")
	bucketTasks = []byte("tasks")
	bucketNotes = []byte("notes")
	keyMeta     = []byte("meta")
)

// Store is a handle to one project's bbolt database.
type Store struct {
	db *bolt.DB
}

// Open opens (creating if needed) the project database at dbPath and ensures
// all buckets and the meta record exist.
func Open(dbPath string) (*Store, error) {
	db, err := bolt.Open(dbPath, 0o600, &bolt.Options{Timeout: time.Second})
	if err != nil {
		return nil, err
	}
	s := &Store{db: db}
	if err := s.init(); err != nil {
		_ = db.Close()
		return nil, err
	}
	return s, nil
}

// Close closes the underlying database.
func (s *Store) Close() error { return s.db.Close() }

func (s *Store) init() error {
	return s.db.Update(func(tx *bolt.Tx) error {
		for _, b := range [][]byte{bucketMeta, bucketPlans, bucketTasks, bucketNotes} {
			if _, err := tx.CreateBucketIfNotExists(b); err != nil {
				return err
			}
		}
		mb := tx.Bucket(bucketMeta)
		raw := mb.Get(keyMeta)
		if raw == nil {
			now := time.Now()
			return putGob(mb, keyMeta, model.Meta{
				CreatedAt:        now,
				UpdatedAt:        now,
				FormatVersion:    CurrentFormat,
				LastWriteVersion: WriterVersion,
			})
		}
		var m model.Meta
		if err := gobDecode(raw, &m); err != nil {
			return err
		}
		if m.FormatVersion > CurrentFormat {
			// Reject without writing: rolling back this transaction discards the
			// bucket creation above, leaving a newer-format DB untouched.
			return ErrFormatTooNew{Found: m.FormatVersion, Supported: CurrentFormat}
		}
		if m.FormatVersion < CurrentFormat {
			migrateMeta(&m)
			m.FormatVersion = CurrentFormat
			m.LastWriteVersion = WriterVersion
			m.UpdatedAt = time.Now()
			return putGob(mb, keyMeta, m)
		}
		return nil
	})
}

// migrateMeta upgrades an older-format meta record in place. FormatVersion 0
// (pre-versioning, v0.1.0) is simply adopted as the current format; future
// steps append here, each guarded by the version they upgrade from.
func migrateMeta(m *model.Meta) {
	// v0 -> v1: nothing to transform; the record shape is compatible.
	_ = m
}

// --- meta ---

// GetMeta returns the project meta record.
func (s *Store) GetMeta() (model.Meta, error) {
	var m model.Meta
	err := s.db.View(func(tx *bolt.Tx) error {
		return getGob(tx.Bucket(bucketMeta), keyMeta, &m)
	})
	return m, err
}

func (s *Store) updateMeta(fn func(*model.Meta)) error {
	return s.db.Update(func(tx *bolt.Tx) error {
		mb := tx.Bucket(bucketMeta)
		var m model.Meta
		if err := getGob(mb, keyMeta, &m); err != nil {
			return err
		}
		fn(&m)
		m.UpdatedAt = time.Now()
		m.LastWriteVersion = WriterVersion
		return putGob(mb, keyMeta, m)
	})
}

// SetGoal sets the project's north-star goal.
func (s *Store) SetGoal(goal string) error {
	return s.updateMeta(func(m *model.Meta) { m.Goal = goal })
}

// SetSummary sets the rolling context summary.
func (s *Store) SetSummary(summary string) error {
	return s.updateMeta(func(m *model.Meta) { m.Summary = summary })
}

// SetActivePlan records which plan is currently active. It verifies the plan
// exists first.
func (s *Store) SetActivePlan(id uint64) error {
	if _, err := s.GetPlan(id); err != nil {
		return err
	}
	return s.updateMeta(func(m *model.Meta) { m.ActivePlan = id })
}

// --- plans ---

// AddPlan appends a new active plan and returns it.
func (s *Store) AddPlan(title string) (model.Plan, error) {
	var p model.Plan
	err := s.db.Update(func(tx *bolt.Tx) error {
		b := tx.Bucket(bucketPlans)
		id, _ := b.NextSequence()
		now := time.Now()
		p = model.Plan{
			ID:        id,
			Title:     title,
			Status:    model.PlanActive,
			Order:     b.Stats().KeyN,
			CreatedAt: now,
			UpdatedAt: now,
		}
		return putGob(b, itob(id), p)
	})
	return p, err
}

// ListPlans returns all plans ordered by Order ascending.
func (s *Store) ListPlans() ([]model.Plan, error) {
	var plans []model.Plan
	err := s.db.View(func(tx *bolt.Tx) error {
		return tx.Bucket(bucketPlans).ForEach(func(_, v []byte) error {
			var p model.Plan
			if err := gobDecode(v, &p); err != nil {
				return err
			}
			plans = append(plans, p)
			return nil
		})
	})
	if err != nil {
		return nil, err
	}
	sortByOrder(plans)
	return plans, nil
}

// GetPlan returns the plan with the given id, or ErrNotFound.
func (s *Store) GetPlan(id uint64) (model.Plan, error) {
	var p model.Plan
	err := s.db.View(func(tx *bolt.Tx) error {
		v := tx.Bucket(bucketPlans).Get(itob(id))
		if v == nil {
			return ErrNotFound
		}
		return gobDecode(v, &p)
	})
	return p, err
}

// SetPlanStatus updates a plan's status.
func (s *Store) SetPlanStatus(id uint64, st model.PlanStatus) error {
	return s.db.Update(func(tx *bolt.Tx) error {
		b := tx.Bucket(bucketPlans)
		var p model.Plan
		if err := getGobNF(b, itob(id), &p); err != nil {
			return err
		}
		p.Status = st
		p.UpdatedAt = time.Now()
		return putGob(b, itob(id), p)
	})
}

// --- tasks ---

// AddTask appends a new todo task to planID and returns it.
func (s *Store) AddTask(planID uint64, title string) (model.Task, error) {
	var t model.Task
	err := s.db.Update(func(tx *bolt.Tx) error {
		if v := tx.Bucket(bucketPlans).Get(itob(planID)); v == nil {
			return ErrNotFound
		}
		b := tx.Bucket(bucketTasks)
		id, _ := b.NextSequence()
		now := time.Now()
		t = model.Task{
			ID:        id,
			PlanID:    planID,
			Title:     title,
			Status:    model.TaskTodo,
			Order:     b.Stats().KeyN,
			CreatedAt: now,
			UpdatedAt: now,
		}
		return putGob(b, itob(id), t)
	})
	return t, err
}

// ListTasks returns all tasks ordered by Order ascending.
func (s *Store) ListTasks() ([]model.Task, error) {
	return s.tasksFilter(func(model.Task) bool { return true })
}

// ListTasksByPlan returns the tasks of one plan, ordered by Order ascending.
func (s *Store) ListTasksByPlan(planID uint64) ([]model.Task, error) {
	return s.tasksFilter(func(t model.Task) bool { return t.PlanID == planID })
}

func (s *Store) tasksFilter(keep func(model.Task) bool) ([]model.Task, error) {
	var tasks []model.Task
	err := s.db.View(func(tx *bolt.Tx) error {
		return tx.Bucket(bucketTasks).ForEach(func(_, v []byte) error {
			var t model.Task
			if err := gobDecode(v, &t); err != nil {
				return err
			}
			if keep(t) {
				tasks = append(tasks, t)
			}
			return nil
		})
	})
	if err != nil {
		return nil, err
	}
	sortByOrder(tasks)
	return tasks, nil
}

// GetTask returns the task with the given id, or ErrNotFound.
func (s *Store) GetTask(id uint64) (model.Task, error) {
	var t model.Task
	err := s.db.View(func(tx *bolt.Tx) error {
		v := tx.Bucket(bucketTasks).Get(itob(id))
		if v == nil {
			return ErrNotFound
		}
		return gobDecode(v, &t)
	})
	return t, err
}

// SetTaskStatus updates a task's status.
func (s *Store) SetTaskStatus(id uint64, st model.TaskStatus) error {
	return s.db.Update(func(tx *bolt.Tx) error {
		b := tx.Bucket(bucketTasks)
		var t model.Task
		if err := getGobNF(b, itob(id), &t); err != nil {
			return err
		}
		t.Status = st
		t.UpdatedAt = time.Now()
		return putGob(b, itob(id), t)
	})
}

// --- notes ---

// AddNote attaches a note to the given target and returns it.
func (s *Store) AddNote(target model.NoteTarget, targetID uint64, body string) (model.Note, error) {
	var n model.Note
	err := s.db.Update(func(tx *bolt.Tx) error {
		b := tx.Bucket(bucketNotes)
		id, _ := b.NextSequence()
		n = model.Note{
			ID:        id,
			Target:    target,
			TargetID:  targetID,
			Body:      body,
			CreatedAt: time.Now(),
		}
		return putGob(b, itob(id), n)
	})
	return n, err
}

// ListNotes returns all notes ordered by CreatedAt ascending (insertion order).
func (s *Store) ListNotes() ([]model.Note, error) {
	var notes []model.Note
	err := s.db.View(func(tx *bolt.Tx) error {
		return tx.Bucket(bucketNotes).ForEach(func(_, v []byte) error {
			var n model.Note
			if err := gobDecode(v, &n); err != nil {
				return err
			}
			notes = append(notes, n)
			return nil
		})
	})
	return notes, err
}

// RecentNotes returns the newest n notes, newest first.
func (s *Store) RecentNotes(n int) ([]model.Note, error) {
	all, err := s.ListNotes()
	if err != nil {
		return nil, err
	}
	// notes are keyed by ascending id == ascending CreatedAt; take the tail.
	if n <= 0 || n > len(all) {
		n = len(all)
	}
	out := make([]model.Note, 0, n)
	for i := len(all) - 1; i >= 0 && len(out) < n; i-- {
		out = append(out, all[i])
	}
	return out, nil
}

// NotesByPlan returns notes attached to the given plan, insertion order.
func (s *Store) NotesByPlan(planID uint64) ([]model.Note, error) {
	return s.notesForTarget(model.TargetPlan, planID)
}

// NotesByTask returns notes attached to the given task, insertion order.
func (s *Store) NotesByTask(taskID uint64) ([]model.Note, error) {
	return s.notesForTarget(model.TargetTask, taskID)
}

func (s *Store) notesForTarget(target model.NoteTarget, id uint64) ([]model.Note, error) {
	all, err := s.ListNotes()
	if err != nil {
		return nil, err
	}
	var out []model.Note
	for _, n := range all {
		if n.Target == target && n.TargetID == id {
			out = append(out, n)
		}
	}
	return out, nil
}

// Counts computes a project-wide inventory in a single pass per bucket, for the
// bounded context footer.
func (s *Store) Counts() (model.Counts, error) {
	var c model.Counts
	err := s.db.View(func(tx *bolt.Tx) error {
		if err := tx.Bucket(bucketPlans).ForEach(func(_, v []byte) error {
			var p model.Plan
			if err := gobDecode(v, &p); err != nil {
				return err
			}
			c.Plans++
			if p.Status == model.PlanDone {
				c.PlansDone++
			}
			return nil
		}); err != nil {
			return err
		}
		if err := tx.Bucket(bucketTasks).ForEach(func(_, v []byte) error {
			var t model.Task
			if err := gobDecode(v, &t); err != nil {
				return err
			}
			c.Tasks++
			switch t.Status {
			case model.TaskDone:
				c.TasksDone++
			case model.TaskBlocked:
				c.TasksBlocked++
			}
			if t.Status.Open() {
				c.TasksOpen++
			}
			return nil
		}); err != nil {
			return err
		}
		c.Notes = tx.Bucket(bucketNotes).Stats().KeyN
		return nil
	})
	return c, err
}
