package store

import (
	"reflect"
	"time"

	"github.com/ro-ag/ptrack/internal/model"
	bolt "go.etcd.io/bbolt"
)

// AddCapability persists a project-local capability and assigns its identity.
func (s *Store) AddCapability(capability model.Capability) (model.Capability, error) {
	err := s.db.Update(func(tx *bolt.Tx) error {
		bucket := tx.Bucket(bucketCapabilities)
		id, _ := bucket.NextSequence()
		now := time.Now()
		capability.ID = id
		if capability.ModelVersion == 0 {
			capability.ModelVersion = model.CapabilityModelVersion
		}
		capability.Revision = 1
		capability.CreatedAt = now
		capability.UpdatedAt = now
		return putGob(bucket, itob(id), capability)
	})
	return capability, err
}

// GetCapability returns one capability by ID.
func (s *Store) GetCapability(id uint64) (model.Capability, error) {
	var capability model.Capability
	err := s.db.View(func(tx *bolt.Tx) error {
		return getGobNF(tx.Bucket(bucketCapabilities), itob(id), &capability)
	})
	return capability, err
}

// ListCapabilities returns capabilities in creation order.
func (s *Store) ListCapabilities() ([]model.Capability, error) {
	var capabilities []model.Capability
	err := s.db.View(func(tx *bolt.Tx) error {
		return tx.Bucket(bucketCapabilities).ForEach(func(_, value []byte) error {
			var capability model.Capability
			if err := gobDecode(value, &capability); err != nil {
				return err
			}
			capabilities = append(capabilities, capability)
			return nil
		})
	})
	return capabilities, err
}

// UpdateCapability replaces the editable fields of an existing capability
// while preserving its identity and creation timestamp.
func (s *Store) UpdateCapability(capability model.Capability) error {
	return s.db.Update(func(tx *bolt.Tx) error {
		bucket := tx.Bucket(bucketCapabilities)
		var existing model.Capability
		if err := getGobNF(bucket, itob(capability.ID), &existing); err != nil {
			return err
		}
		materialChanged := !reflect.DeepEqual(capabilitySecurityEnvelope(existing), capabilitySecurityEnvelope(capability))
		capability.ID = existing.ID
		capability.ModelVersion = existing.ModelVersion
		capability.Revision = existing.Revision + 1
		capability.CreatedAt = existing.CreatedAt
		if materialChanged {
			capability.Enabled = false
			capability.ApprovedAt = time.Time{}
			capability.ExpiresAt = time.Time{}
		}
		capability.UpdatedAt = time.Now()
		return putGob(bucket, itob(capability.ID), capability)
	})
}

// DeleteCapability removes a grant. Bounded audit metadata is deliberately
// retained as a tombstoned history and remains queryable by capability ID.
func (s *Store) DeleteCapability(id uint64) error {
	return s.db.Update(func(tx *bolt.Tx) error {
		capabilities := tx.Bucket(bucketCapabilities)
		if capabilities.Get(itob(id)) == nil {
			return ErrNotFound
		}
		return capabilities.Delete(itob(id))
	})
}

type capabilityEnvelope struct {
	Kind                    model.CapabilityKind
	AgentProfile            string
	ApprovalDurationSeconds int64
	Limits                  model.CapabilityLimits
	Audit                   model.CapabilityAuditPolicy
	HTTP                    *model.HTTPScope
	Git                     *model.GitScope
	SSH                     *model.SSHScope
}

func capabilitySecurityEnvelope(capability model.Capability) capabilityEnvelope {
	return capabilityEnvelope{
		Kind:                    capability.Kind,
		AgentProfile:            capability.AgentProfile,
		ApprovalDurationSeconds: capability.ApprovalDurationSeconds,
		Limits:                  capability.Limits,
		Audit:                   capability.Audit,
		HTTP:                    capability.HTTP,
		Git:                     capability.Git,
		SSH:                     capability.SSH,
	}
}

// AddCapabilityAudit appends metadata for one operation.
func (s *Store) AddCapabilityAudit(audit model.CapabilityAudit) (model.CapabilityAudit, error) {
	return s.AddCapabilityAuditBounded(audit, 0, 0)
}

// AddCapabilityAuditBounded appends metadata and prunes old records in one
// transaction. Non-positive ceilings mean unlimited for that dimension.
func (s *Store) AddCapabilityAuditBounded(
	audit model.CapabilityAudit,
	perCapabilityKeep, totalKeep int,
) (model.CapabilityAudit, error) {
	err := s.db.Update(func(tx *bolt.Tx) error {
		bucket := tx.Bucket(bucketCapabilityAudits)
		id, _ := bucket.NextSequence()
		audit.ID = id
		if audit.CreatedAt.IsZero() {
			audit.CreatedAt = time.Now()
		}
		if err := putGob(bucket, itob(id), audit); err != nil {
			return err
		}
		return pruneAudits(bucket, audit.CapabilityID, perCapabilityKeep, totalKeep)
	})
	return audit, err
}

func pruneAudits(bucket *bolt.Bucket, capabilityID uint64, perCapabilityKeep, totalKeep int) error {
	cursor := bucket.Cursor()
	matching := 0
	total := 0
	for _, value := cursor.Last(); value != nil; _, value = cursor.Prev() {
		var audit model.CapabilityAudit
		if err := gobDecode(value, &audit); err != nil {
			return err
		}
		total++
		remove := totalKeep > 0 && total > totalKeep
		if audit.CapabilityID == capabilityID {
			matching++
			remove = remove || perCapabilityKeep > 0 && matching > perCapabilityKeep
		}
		if remove {
			if err := cursor.Delete(); err != nil {
				return err
			}
		}
	}
	return nil
}

// ListCapabilityAudits returns newest-first audit records, optionally scoped
// to one capability. A non-positive limit returns all matching records.
func (s *Store) ListCapabilityAudits(capabilityID uint64, limit int) ([]model.CapabilityAudit, error) {
	var audits []model.CapabilityAudit
	err := s.db.View(func(tx *bolt.Tx) error {
		cursor := tx.Bucket(bucketCapabilityAudits).Cursor()
		for _, value := cursor.Last(); value != nil; _, value = cursor.Prev() {
			var audit model.CapabilityAudit
			if err := gobDecode(value, &audit); err != nil {
				return err
			}
			if capabilityID != 0 && audit.CapabilityID != capabilityID {
				continue
			}
			audits = append(audits, audit)
			if limit > 0 && len(audits) >= limit {
				break
			}
		}
		return nil
	})
	return audits, err
}

// PruneCapabilityAudits retains only the newest keep records for a capability.
func (s *Store) PruneCapabilityAudits(capabilityID uint64, keep int) error {
	if keep < 0 {
		keep = 0
	}
	return s.db.Update(func(tx *bolt.Tx) error {
		return pruneAudits(tx.Bucket(bucketCapabilityAudits), capabilityID, keep, 0)
	})
}
