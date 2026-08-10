package store

import (
	"bytes"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"fmt"
	"time"
	"unicode/utf8"

	"github.com/ro-ag/ptrack/internal/model"
	bolt "go.etcd.io/bbolt"
)

const (
	// MemoryWritebackReplayLimit bounds persisted request identities per project.
	MemoryWritebackReplayLimit  = 256
	maxMemoryWritebackRequestID = 128
)

var (
	// ErrMemoryWritebackReplay is returned when a request ID is reused for
	// different content or a different derived target.
	ErrMemoryWritebackReplay = errors.New("memory write-back request ID was already used")
	// ErrInvalidMemoryWriteback rejects malformed internal write requests.
	ErrInvalidMemoryWriteback = errors.New("invalid memory write-back")
)

// MemoryWriteRequest is an internal, already content-validated request. Target
// is derived from a live host-owned association by the GUI service; PlanID is
// retained separately so task ownership is revalidated in this transaction.
type MemoryWriteRequest struct {
	RequestID           string
	Kind                model.MemoryKind
	Body                string
	Target              model.NoteTarget
	TargetID            uint64
	PlanID              uint64
	WorkspaceGeneration uint64
	SessionID           string
	AssociationRevision uint64
}

// MemoryWriteResult describes the durable mutation. Note is set for typed
// memory; summary writes update Meta instead. Replayed means no new mutation
// occurred because an identical request ID had already committed.
type MemoryWriteResult struct {
	Note     *model.Note
	Summary  string
	Replayed bool
}

type memoryWritebackRecord struct {
	Digest   [sha256.Size]byte
	Sequence uint64
	Kind     model.MemoryKind
	NoteID   uint64
}

// WriteMemory atomically revalidates the derived target, applies the mutation,
// and records bounded replay protection. It deliberately performs no status or
// issue mutation.
func (s *Store) WriteMemory(request MemoryWriteRequest) (MemoryWriteResult, error) {
	if err := validateMemoryWriteRequest(request); err != nil {
		return MemoryWriteResult{}, err
	}
	digest, err := memoryWriteDigest(request)
	if err != nil {
		return MemoryWriteResult{}, err
	}
	var result MemoryWriteResult
	err = s.db.Update(func(tx *bolt.Tx) error {
		replays := tx.Bucket(bucketMemoryWritebacks)
		if raw := replays.Get([]byte(request.RequestID)); raw != nil {
			var record memoryWritebackRecord
			if decodeErr := gobDecode(raw, &record); decodeErr != nil {
				return decodeErr
			}
			if !bytes.Equal(record.Digest[:], digest[:]) {
				return ErrMemoryWritebackReplay
			}
			result.Replayed = true
			if record.Kind == model.MemorySummary {
				result.Summary = request.Body
			} else {
				var note model.Note
				if err := getGobNF(tx.Bucket(bucketNotes), itob(record.NoteID), &note); err != nil {
					return err
				}
				result.Note = &note
			}
			return nil
		}

		if err := validateMemoryTargetTx(tx, request); err != nil {
			return err
		}
		record := memoryWritebackRecord{Digest: digest, Kind: request.Kind}
		record.Sequence, _ = replays.NextSequence()
		switch request.Kind {
		case model.MemorySummary:
			metaBucket := tx.Bucket(bucketMeta)
			var meta model.Meta
			if err := getGob(metaBucket, keyMeta, &meta); err != nil {
				return err
			}
			meta.Summary = request.Body
			meta.UpdatedAt = time.Now().UTC()
			meta.LastWriteVersion = WriterVersion
			if err := putGob(metaBucket, keyMeta, meta); err != nil {
				return err
			}
			result.Summary = request.Body
		case model.MemoryDecision, model.MemoryBlocker, model.MemoryHandoff:
			notes := tx.Bucket(bucketNotes)
			id, _ := notes.NextSequence()
			note := model.Note{
				ID:        id,
				Target:    request.Target,
				TargetID:  request.TargetID,
				Kind:      request.Kind,
				Body:      request.Body,
				CreatedAt: time.Now().UTC(),
			}
			if err := putGob(notes, itob(id), note); err != nil {
				return err
			}
			record.NoteID = note.ID
			result.Note = cloneMemoryNote(&note)
		default:
			return ErrInvalidMemoryWriteback
		}
		if err := putGob(replays, []byte(request.RequestID), record); err != nil {
			return err
		}
		return pruneMemoryWritebacks(replays)
	})
	return result, err
}

func validateMemoryWriteRequest(request MemoryWriteRequest) error {
	if request.RequestID == "" || len(request.RequestID) > maxMemoryWritebackRequestID ||
		!utf8.ValidString(request.RequestID) {
		return fmt.Errorf("%w: invalid request ID", ErrInvalidMemoryWriteback)
	}
	for _, value := range request.RequestID {
		if !(value >= 'a' && value <= 'z' || value >= 'A' && value <= 'Z' ||
			value >= '0' && value <= '9' || value == '-' || value == '_' ||
			value == '.' || value == ':') {
			return fmt.Errorf("%w: invalid request ID", ErrInvalidMemoryWriteback)
		}
	}
	if request.Body == "" || !utf8.ValidString(request.Body) {
		return fmt.Errorf("%w: content is required", ErrInvalidMemoryWriteback)
	}
	if request.WorkspaceGeneration == 0 || request.SessionID == "" ||
		len(request.SessionID) > maxMemoryWritebackRequestID ||
		request.AssociationRevision == 0 {
		return fmt.Errorf("%w: source association is required", ErrInvalidMemoryWriteback)
	}
	switch request.Kind {
	case model.MemorySummary, model.MemoryDecision, model.MemoryBlocker, model.MemoryHandoff:
	default:
		return fmt.Errorf("%w: unsupported kind %q", ErrInvalidMemoryWriteback, request.Kind)
	}
	return nil
}

func validateMemoryTargetTx(tx *bolt.Tx, request MemoryWriteRequest) error {
	switch request.Target {
	case model.TargetProject:
		if request.TargetID != 0 || request.PlanID != 0 {
			return fmt.Errorf("%w: invalid project target", ErrInvalidMemoryWriteback)
		}
	case model.TargetPlan:
		if request.TargetID == 0 || request.PlanID != request.TargetID ||
			tx.Bucket(bucketPlans).Get(itob(request.TargetID)) == nil {
			return fmt.Errorf("%w: plan target no longer exists", ErrInvalidMemoryWriteback)
		}
	case model.TargetTask:
		if request.TargetID == 0 || request.PlanID == 0 {
			return fmt.Errorf("%w: invalid task target", ErrInvalidMemoryWriteback)
		}
		var task model.Task
		if err := getGobNF(tx.Bucket(bucketTasks), itob(request.TargetID), &task); err != nil {
			return fmt.Errorf("%w: task target no longer exists", ErrInvalidMemoryWriteback)
		}
		if task.PlanID != request.PlanID ||
			tx.Bucket(bucketPlans).Get(itob(request.PlanID)) == nil {
			return fmt.Errorf("%w: task target changed plans", ErrInvalidMemoryWriteback)
		}
	default:
		return fmt.Errorf("%w: unsupported target %q", ErrInvalidMemoryWriteback, request.Target)
	}
	return nil
}

func memoryWriteDigest(request MemoryWriteRequest) ([sha256.Size]byte, error) {
	encoded, err := json.Marshal(struct {
		Kind       model.MemoryKind `json:"kind"`
		Body       string           `json:"body"`
		Target     model.NoteTarget `json:"target"`
		TargetID   uint64           `json:"target_id"`
		PlanID     uint64           `json:"plan_id"`
		Generation uint64           `json:"generation"`
		SessionID  string           `json:"session_id"`
		Revision   uint64           `json:"revision"`
	}{
		request.Kind, request.Body, request.Target, request.TargetID,
		request.PlanID, request.WorkspaceGeneration, request.SessionID,
		request.AssociationRevision,
	})
	if err != nil {
		return [sha256.Size]byte{}, err
	}
	return sha256.Sum256(encoded), nil
}

func pruneMemoryWritebacks(bucket *bolt.Bucket) error {
	count := 0
	var oldestKey []byte
	var oldestSequence = ^uint64(0)
	if err := bucket.ForEach(func(key, value []byte) error {
		count++
		var record memoryWritebackRecord
		if err := gobDecode(value, &record); err != nil {
			return err
		}
		if record.Sequence < oldestSequence {
			oldestSequence = record.Sequence
			oldestKey = append(oldestKey[:0], key...)
		}
		return nil
	}); err != nil {
		return err
	}
	if count <= MemoryWritebackReplayLimit {
		return nil
	}
	if oldestKey == nil {
		return nil
	}
	return bucket.Delete(oldestKey)
}

func cloneMemoryNote(note *model.Note) *model.Note {
	if note == nil {
		return nil
	}
	copy := *note
	return &copy
}
