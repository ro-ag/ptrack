package store

import (
	"time"

	"github.com/ro-ag/ptrack/internal/model"
	bolt "go.etcd.io/bbolt"
)

// AddCommit records a git commit, linked to a task and/or plan. It is idempotent
// by SHA: recording the same SHA again returns the existing record unchanged.
func (s *Store) AddCommit(sha, subject string, planID, taskID uint64) (model.Commit, error) {
	var c model.Commit
	err := s.db.Update(func(tx *bolt.Tx) error {
		b := tx.Bucket(bucketCommits)
		// Dedup by SHA.
		var existing *model.Commit
		if err := b.ForEach(func(_, v []byte) error {
			var cur model.Commit
			if err := gobDecode(v, &cur); err != nil {
				return err
			}
			if cur.SHA == sha {
				existing = &cur
			}
			return nil
		}); err != nil {
			return err
		}
		if existing != nil {
			c = *existing
			return nil
		}
		id, _ := b.NextSequence()
		c = model.Commit{
			ID:        id,
			SHA:       sha,
			Subject:   subject,
			PlanID:    planID,
			TaskID:    taskID,
			CreatedAt: time.Now(),
		}
		return putGob(b, itob(id), c)
	})
	return c, err
}

// ListCommits returns all commits in insertion order.
func (s *Store) ListCommits() ([]model.Commit, error) {
	var out []model.Commit
	err := s.db.View(func(tx *bolt.Tx) error {
		return tx.Bucket(bucketCommits).ForEach(func(_, v []byte) error {
			var c model.Commit
			if err := gobDecode(v, &c); err != nil {
				return err
			}
			out = append(out, c)
			return nil
		})
	})
	return out, err
}

// CommitsByTask returns commits linked to a task, newest first.
func (s *Store) CommitsByTask(taskID uint64) ([]model.Commit, error) {
	return s.commitsFilter(func(c model.Commit) bool { return c.TaskID == taskID })
}

// CommitsByPlan returns commits linked to a plan, newest first.
func (s *Store) CommitsByPlan(planID uint64) ([]model.Commit, error) {
	return s.commitsFilter(func(c model.Commit) bool { return c.PlanID == planID })
}

func (s *Store) commitsFilter(keep func(model.Commit) bool) ([]model.Commit, error) {
	all, err := s.ListCommits()
	if err != nil {
		return nil, err
	}
	var out []model.Commit
	for i := len(all) - 1; i >= 0; i-- {
		if keep(all[i]) {
			out = append(out, all[i])
		}
	}
	return out, nil
}
