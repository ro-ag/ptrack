package main

import (
	"crypto/sha256"
	"errors"
	"fmt"

	"github.com/ro-ag/ptrack/internal/model"
)

// MigrationKind identifies one of the two frozen legacy bbolt schemas.
type MigrationKind uint8

const (
	MigrationKindProject MigrationKind = 1
	MigrationKindGlobal  MigrationKind = 2

	migrationMaxRecords         = 1_000_000
	migrationMaxKeyBytes        = 1 << 20
	migrationMaxValueBytes      = 256 << 20
	migrationMaxPayloadBytes    = uint64(256) << 20
	migrationRecordOverhead     = 20
	maxMemoryWritebackRequestID = 128

	// CurrentFormat is the final supported legacy project schema.
	CurrentFormat uint = 5
)

type migrationBucketSpec struct {
	name       string
	sequenced  bool
	numericKey bool
	introduced uint64
}

var projectMigrationBuckets = []migrationBucketSpec{
	{name: "meta", introduced: 0},
	{name: "plans", sequenced: true, numericKey: true, introduced: 0},
	{name: "tasks", sequenced: true, numericKey: true, introduced: 0},
	{name: "notes", sequenced: true, numericKey: true, introduced: 0},
	{name: "milestones", sequenced: true, numericKey: true, introduced: 2},
	{name: "issues", sequenced: true, numericKey: true, introduced: 2},
	{name: "commits", sequenced: true, numericKey: true, introduced: 3},
	{name: "capabilities", sequenced: true, numericKey: true, introduced: 4},
	{name: "capability_audits", sequenced: true, numericKey: true, introduced: 4},
	{name: "memory_writebacks", sequenced: true, introduced: 5},
}

var globalMigrationBuckets = []migrationBucketSpec{
	{name: "config"},
	{name: "projects"},
	{name: "backups"},
}

var (
	bucketMeta             = []byte("meta")
	bucketPlans            = []byte("plans")
	bucketTasks            = []byte("tasks")
	bucketNotes            = []byte("notes")
	bucketMilestones       = []byte("milestones")
	bucketIssues           = []byte("issues")
	bucketCommits          = []byte("commits")
	bucketCapabilities     = []byte("capabilities")
	bucketCapabilityAudits = []byte("capability_audits")
	bucketMemoryWritebacks = []byte("memory_writebacks")
	bucketConfig           = []byte("config")
	bucketProjects         = []byte("projects")
	bucketBackups          = []byte("backups")
	keyMeta                = []byte("meta")
)

type memoryWritebackRecord struct {
	Digest   [sha256.Size]byte
	Sequence uint64
	Kind     model.MemoryKind
	NoteID   uint64
}

// ErrFormatTooNew rejects a legacy database produced by an unknown schema.
type ErrFormatTooNew struct {
	Found     uint
	Supported uint
}

func (e ErrFormatTooNew) Error() string {
	return fmt.Sprintf("database format v%d is newer than this ptrack (supports v%d) — upgrade ptrack", e.Found, e.Supported)
}

func drainMigrationCheck(checkErrors <-chan error) error {
	if checkErrors == nil {
		return errors.New("bbolt integrity check returned no result channel")
	}
	var integrityErr error
	for err := range checkErrors {
		if err != nil {
			integrityErr = errors.Join(integrityErr, err)
		}
	}
	if integrityErr != nil {
		return fmt.Errorf("bbolt integrity check failed: %w", integrityErr)
	}
	return nil
}
