package store

import (
	"context"
	"errors"

	"github.com/ro-ag/ptrack/internal/model"
	bolt "go.etcd.io/bbolt"
)

const maxBoundedRead = 1_000

type Bounded[T any] struct {
	Items []T `json:"items"`
	Total int `json:"total"`
	More  int `json:"more"`
}

// ScanBounded is a deterministic newest-first hard scan. At most ScanLimit
// records are decoded, and Truncated reports that at least one older record
// was deliberately not inspected.
type ScanBounded[T any] struct {
	Items     []T
	Scanned   int
	ScanLimit int
	Truncated bool
}

type TaskProgress struct {
	Total int `json:"total"`
	Done  int `json:"done"`
}

type TaskAssociations struct {
	NoteCounts   map[uint64]int
	CommitCounts map[uint64]int
	IssueCounts  map[uint64]int
	LatestNotes  map[uint64]string
}

func validateBoundedLimit(limit int) error {
	if limit <= 0 || limit > maxBoundedRead {
		return errors.New("bounded read limit must be between 1 and 1000")
	}
	return nil
}

func bounded[T any](items []T, total int) Bounded[T] {
	return Bounded[T]{
		Items: items,
		Total: total,
		More:  max(0, total-len(items)),
	}
}

func (s *Store) ListPlansBounded(limit int) (Bounded[model.Plan], error) {
	if err := validateBoundedLimit(limit); err != nil {
		return Bounded[model.Plan]{}, err
	}
	items := make([]model.Plan, 0, limit)
	total := 0
	err := s.db.View(func(tx *bolt.Tx) error {
		bucket := tx.Bucket(bucketPlans)
		total = bucket.Stats().KeyN
		cursor := bucket.Cursor()
		for _, value := cursor.First(); value != nil && len(items) < limit; _, value = cursor.Next() {
			var plan model.Plan
			if err := gobDecode(value, &plan); err != nil {
				return err
			}
			items = append(items, plan)
		}
		return nil
	})
	return bounded(items, total), err
}

func (s *Store) ListTasksByPlanBounded(
	planID uint64,
	limit int,
) (Bounded[model.Task], error) {
	return s.ListTasksByPlanBoundedContext(context.Background(), planID, limit)
}

func (s *Store) ListTasksByPlanBoundedContext(
	ctx context.Context,
	planID uint64,
	limit int,
) (Bounded[model.Task], error) {
	return s.boundedTasks(ctx, limit, func(task model.Task) bool {
		return task.PlanID == planID
	})
}

func (s *Store) ListBlockedTasksBounded(
	limit int,
) (Bounded[model.Task], error) {
	return s.ListBlockedTasksBoundedContext(context.Background(), limit)
}

func (s *Store) ListBlockedTasksBoundedContext(
	ctx context.Context,
	limit int,
) (Bounded[model.Task], error) {
	return s.boundedTasks(ctx, limit, func(task model.Task) bool {
		return task.Status == model.TaskBlocked
	})
}

func (s *Store) PlanTaskProgress(planID uint64) (TaskProgress, error) {
	return s.PlanTaskProgressContext(context.Background(), planID)
}

func (s *Store) PlanTaskProgressContext(
	ctx context.Context,
	planID uint64,
) (TaskProgress, error) {
	if err := ctx.Err(); err != nil {
		return TaskProgress{}, err
	}
	var progress TaskProgress
	err := s.db.View(func(tx *bolt.Tx) error {
		return tx.Bucket(bucketTasks).ForEach(func(_, value []byte) error {
			if err := ctx.Err(); err != nil {
				return err
			}
			var task model.Task
			if err := gobDecode(value, &task); err != nil {
				return err
			}
			if task.PlanID != planID {
				return nil
			}
			progress.Total++
			if task.Status == model.TaskDone {
				progress.Done++
			}
			return nil
		})
	})
	return progress, err
}

func (s *Store) boundedTasks(
	ctx context.Context,
	limit int,
	keep func(model.Task) bool,
) (Bounded[model.Task], error) {
	if err := ctx.Err(); err != nil {
		return Bounded[model.Task]{}, err
	}
	if err := validateBoundedLimit(limit); err != nil {
		return Bounded[model.Task]{}, err
	}
	items := make([]model.Task, 0, limit)
	total := 0
	err := s.db.View(func(tx *bolt.Tx) error {
		return tx.Bucket(bucketTasks).ForEach(func(_, value []byte) error {
			if err := ctx.Err(); err != nil {
				return err
			}
			var task model.Task
			if err := gobDecode(value, &task); err != nil {
				return err
			}
			if keep(task) {
				total++
				if len(items) < limit {
					items = append(items, task)
				}
			}
			return nil
		})
	})
	return bounded(items, total), err
}

func (s *Store) RecentNotesBounded(limit int) (Bounded[model.Note], error) {
	if err := validateBoundedLimit(limit); err != nil {
		return Bounded[model.Note]{}, err
	}
	items := make([]model.Note, 0, limit)
	total := 0
	err := s.db.View(func(tx *bolt.Tx) error {
		bucket := tx.Bucket(bucketNotes)
		total = bucket.Stats().KeyN
		cursor := bucket.Cursor()
		for _, value := cursor.Last(); value != nil && len(items) < limit; _, value = cursor.Prev() {
			var note model.Note
			if err := gobDecode(value, &note); err != nil {
				return err
			}
			items = append(items, note)
		}
		return nil
	})
	return bounded(items, total), err
}

func (s *Store) RecentCommitsBounded(limit int) (Bounded[model.Commit], error) {
	if err := validateBoundedLimit(limit); err != nil {
		return Bounded[model.Commit]{}, err
	}
	items := make([]model.Commit, 0, limit)
	total := 0
	err := s.db.View(func(tx *bolt.Tx) error {
		bucket := tx.Bucket(bucketCommits)
		total = bucket.Stats().KeyN
		cursor := bucket.Cursor()
		for _, value := cursor.Last(); value != nil && len(items) < limit; _, value = cursor.Prev() {
			var commit model.Commit
			if err := gobDecode(value, &commit); err != nil {
				return err
			}
			items = append(items, commit)
		}
		return nil
	})
	return bounded(items, total), err
}

func (s *Store) ListOpenIssuesBounded(limit int) (Bounded[model.Issue], error) {
	return s.ListOpenIssuesBoundedContext(context.Background(), limit)
}

// ListOpenIssuesScanBounded returns open issues found among at most scanLimit
// newest issue records. Unlike ListOpenIssuesBounded, it intentionally does
// not traverse older records to compute an exact open-issue total.
func (s *Store) ListOpenIssuesScanBounded(
	scanLimit int,
) (ScanBounded[model.Issue], error) {
	if err := validateBoundedLimit(scanLimit); err != nil {
		return ScanBounded[model.Issue]{}, err
	}
	result := ScanBounded[model.Issue]{
		Items:     make([]model.Issue, 0, scanLimit),
		ScanLimit: scanLimit,
	}
	err := s.db.View(func(tx *bolt.Tx) error {
		bucket := tx.Bucket(bucketIssues)
		cursor := bucket.Cursor()
		_, value := cursor.Last()
		for value != nil && result.Scanned < scanLimit {
			result.Scanned++
			var issue model.Issue
			if err := gobDecode(value, &issue); err != nil {
				return err
			}
			if issue.Status == model.IssueOpen {
				result.Items = append(result.Items, issue)
			}
			_, value = cursor.Prev()
		}
		result.Truncated = value != nil
		return nil
	})
	return result, err
}

func (s *Store) ListOpenIssuesBoundedContext(
	ctx context.Context,
	limit int,
) (Bounded[model.Issue], error) {
	if err := ctx.Err(); err != nil {
		return Bounded[model.Issue]{}, err
	}
	if err := validateBoundedLimit(limit); err != nil {
		return Bounded[model.Issue]{}, err
	}
	items := make([]model.Issue, 0, limit)
	total := 0
	err := s.db.View(func(tx *bolt.Tx) error {
		cursor := tx.Bucket(bucketIssues).Cursor()
		for _, value := cursor.Last(); value != nil; _, value = cursor.Prev() {
			if err := ctx.Err(); err != nil {
				return err
			}
			var issue model.Issue
			if err := gobDecode(value, &issue); err != nil {
				return err
			}
			if issue.Status == model.IssueOpen {
				total++
				if len(items) < limit {
					items = append(items, issue)
				}
			}
		}
		return nil
	})
	return bounded(items, total), err
}

func (s *Store) TaskAssociationsContext(
	ctx context.Context,
	taskIDs map[uint64]bool,
) (TaskAssociations, error) {
	associations := TaskAssociations{
		NoteCounts:   make(map[uint64]int, len(taskIDs)),
		CommitCounts: make(map[uint64]int, len(taskIDs)),
		IssueCounts:  make(map[uint64]int, len(taskIDs)),
		LatestNotes:  make(map[uint64]string, len(taskIDs)),
	}
	if err := ctx.Err(); err != nil {
		return associations, err
	}
	err := s.db.View(func(tx *bolt.Tx) error {
		noteCursor := tx.Bucket(bucketNotes).Cursor()
		for _, value := noteCursor.Last(); value != nil; _, value = noteCursor.Prev() {
			if err := ctx.Err(); err != nil {
				return err
			}
			var note model.Note
			if err := gobDecode(value, &note); err != nil {
				return err
			}
			if note.Target != model.TargetTask || !taskIDs[note.TargetID] {
				continue
			}
			associations.NoteCounts[note.TargetID]++
			if _, exists := associations.LatestNotes[note.TargetID]; !exists {
				associations.LatestNotes[note.TargetID] = note.Body
			}
		}
		if err := tx.Bucket(bucketCommits).ForEach(func(_, value []byte) error {
			if err := ctx.Err(); err != nil {
				return err
			}
			var commit model.Commit
			if err := gobDecode(value, &commit); err != nil {
				return err
			}
			if taskIDs[commit.TaskID] {
				associations.CommitCounts[commit.TaskID]++
			}
			return nil
		}); err != nil {
			return err
		}
		return tx.Bucket(bucketIssues).ForEach(func(_, value []byte) error {
			if err := ctx.Err(); err != nil {
				return err
			}
			var issue model.Issue
			if err := gobDecode(value, &issue); err != nil {
				return err
			}
			if issue.Status == model.IssueOpen && taskIDs[issue.TaskID] {
				associations.IssueCounts[issue.TaskID]++
			}
			return nil
		})
	})
	return associations, err
}
