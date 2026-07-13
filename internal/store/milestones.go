package store

import (
	"time"

	"github.com/ro-ag/ptrack/internal/model"
	bolt "go.etcd.io/bbolt"
)

// --- milestones ---

// AddMilestone appends a new open milestone and returns it.
func (s *Store) AddMilestone(title string) (model.Milestone, error) {
	var m model.Milestone
	err := s.db.Update(func(tx *bolt.Tx) error {
		b := tx.Bucket(bucketMilestones)
		id, _ := b.NextSequence()
		now := time.Now()
		m = model.Milestone{
			ID:        id,
			Title:     title,
			Status:    model.MilestoneOpen,
			Order:     b.Stats().KeyN,
			CreatedAt: now,
			UpdatedAt: now,
		}
		return putGob(b, itob(id), m)
	})
	return m, err
}

// ListMilestones returns all milestones ordered by Order ascending.
func (s *Store) ListMilestones() ([]model.Milestone, error) {
	var ms []model.Milestone
	err := s.db.View(func(tx *bolt.Tx) error {
		return tx.Bucket(bucketMilestones).ForEach(func(_, v []byte) error {
			var m model.Milestone
			if err := gobDecode(v, &m); err != nil {
				return err
			}
			ms = append(ms, m)
			return nil
		})
	})
	if err != nil {
		return nil, err
	}
	sortByOrder(ms)
	return ms, nil
}

// GetMilestone returns the milestone with the given id, or ErrNotFound.
func (s *Store) GetMilestone(id uint64) (model.Milestone, error) {
	var m model.Milestone
	err := s.db.View(func(tx *bolt.Tx) error {
		v := tx.Bucket(bucketMilestones).Get(itob(id))
		if v == nil {
			return ErrNotFound
		}
		return gobDecode(v, &m)
	})
	return m, err
}

// SetMilestoneStatus updates a milestone's status.
func (s *Store) SetMilestoneStatus(id uint64, st model.MilestoneStatus) error {
	return s.mutateMilestone(id, func(m *model.Milestone) { m.Status = st })
}

// SetMilestoneDue sets (or clears, with the zero time) a milestone's due date.
func (s *Store) SetMilestoneDue(id uint64, due time.Time) error {
	return s.mutateMilestone(id, func(m *model.Milestone) { m.Due = due })
}

// SetMilestoneTitle renames a milestone.
func (s *Store) SetMilestoneTitle(id uint64, title string) error {
	return s.mutateMilestone(id, func(m *model.Milestone) { m.Title = title })
}

func (s *Store) mutateMilestone(id uint64, fn func(*model.Milestone)) error {
	return s.db.Update(func(tx *bolt.Tx) error {
		b := tx.Bucket(bucketMilestones)
		var m model.Milestone
		if err := getGobNF(b, itob(id), &m); err != nil {
			return err
		}
		fn(&m)
		m.UpdatedAt = time.Now()
		return putGob(b, itob(id), m)
	})
}

// --- plan ↔ milestone ---

// SetPlanMilestone links a plan to a milestone (0 unlinks). It verifies both
// exist.
func (s *Store) SetPlanMilestone(planID, milestoneID uint64) error {
	return s.db.Update(func(tx *bolt.Tx) error {
		if milestoneID != 0 {
			if tx.Bucket(bucketMilestones).Get(itob(milestoneID)) == nil {
				return ErrNotFound
			}
		}
		b := tx.Bucket(bucketPlans)
		var p model.Plan
		if err := getGobNF(b, itob(planID), &p); err != nil {
			return err
		}
		p.MilestoneID = milestoneID
		p.UpdatedAt = time.Now()
		return putGob(b, itob(planID), p)
	})
}

// ListPlansByMilestone returns the plans assigned to a milestone, ordered.
func (s *Store) ListPlansByMilestone(milestoneID uint64) ([]model.Plan, error) {
	all, err := s.ListPlans()
	if err != nil {
		return nil, err
	}
	var out []model.Plan
	for _, p := range all {
		if p.MilestoneID == milestoneID {
			out = append(out, p)
		}
	}
	return out, nil
}

// --- issues ---

// AddIssue creates a new open issue and returns it. severity defaults to medium
// when empty; taskID 0 leaves it unlinked.
func (s *Store) AddIssue(title, body string, severity model.Severity, taskID uint64) (model.Issue, error) {
	if severity == "" {
		severity = model.SeverityMedium
	}
	var is model.Issue
	err := s.db.Update(func(tx *bolt.Tx) error {
		if taskID != 0 && tx.Bucket(bucketTasks).Get(itob(taskID)) == nil {
			return ErrNotFound
		}
		b := tx.Bucket(bucketIssues)
		id, _ := b.NextSequence()
		now := time.Now()
		is = model.Issue{
			ID:        id,
			Title:     title,
			Body:      body,
			Status:    model.IssueOpen,
			Severity:  severity,
			TaskID:    taskID,
			CreatedAt: now,
			UpdatedAt: now,
		}
		return putGob(b, itob(id), is)
	})
	return is, err
}

// ListIssues returns all issues ordered by id ascending.
func (s *Store) ListIssues() ([]model.Issue, error) {
	var out []model.Issue
	err := s.db.View(func(tx *bolt.Tx) error {
		return tx.Bucket(bucketIssues).ForEach(func(_, v []byte) error {
			var is model.Issue
			if err := gobDecode(v, &is); err != nil {
				return err
			}
			out = append(out, is)
			return nil
		})
	})
	return out, err
}

// GetIssue returns the issue with the given id, or ErrNotFound.
func (s *Store) GetIssue(id uint64) (model.Issue, error) {
	var is model.Issue
	err := s.db.View(func(tx *bolt.Tx) error {
		v := tx.Bucket(bucketIssues).Get(itob(id))
		if v == nil {
			return ErrNotFound
		}
		return gobDecode(v, &is)
	})
	return is, err
}

// SetIssueStatus updates an issue's status.
func (s *Store) SetIssueStatus(id uint64, st model.IssueStatus) error {
	return s.mutateIssue(id, func(is *model.Issue) { is.Status = st })
}

// SetIssueSeverity updates an issue's severity.
func (s *Store) SetIssueSeverity(id uint64, sev model.Severity) error {
	return s.mutateIssue(id, func(is *model.Issue) { is.Severity = sev })
}

// SetIssueTitle renames an issue.
func (s *Store) SetIssueTitle(id uint64, title string) error {
	return s.mutateIssue(id, func(is *model.Issue) { is.Title = title })
}

func (s *Store) mutateIssue(id uint64, fn func(*model.Issue)) error {
	return s.db.Update(func(tx *bolt.Tx) error {
		b := tx.Bucket(bucketIssues)
		var is model.Issue
		if err := getGobNF(b, itob(id), &is); err != nil {
			return err
		}
		fn(&is)
		is.UpdatedAt = time.Now()
		return putGob(b, itob(id), is)
	})
}
