//go:build !windows

package main

import (
	"bytes"
	"encoding/binary"
	"encoding/gob"
	"encoding/hex"
	"fmt"
	"os"
	"path/filepath"
	"testing"

	"github.com/ro-ag/ptrack/internal/model"
	bolt "go.etcd.io/bbolt"
)

const crossLanguageFixturePrefix = "PTRACK_XLANG_FIXTURE"

// TestCrossLanguageFixtures is intentionally selected by the Rust acceptance
// test. It creates only disposable bbolt databases and exports them through the
// real command path; the stable markers are the sole cross-language interface.
func TestCrossLanguageFixtures(t *testing.T) {
	directory := t.TempDir()
	fixtures := []struct {
		kind         string
		sourceFormat uint64
		bucketCount  uint64
		recordCount  uint64
		create       func(*testing.T, string)
	}{
		{
			kind:         "project",
			sourceFormat: 5,
			bucketCount:  10,
			recordCount:  10,
			create:       createCrossLanguageProject,
		},
		{
			kind:         "global",
			sourceFormat: 0,
			bucketCount:  3,
			recordCount:  3,
			create:       createCrossLanguageGlobal,
		},
	}

	for _, fixture := range fixtures {
		source := filepath.Join(directory, fixture.kind+".db")
		bundle := filepath.Join(directory, fixture.kind+".bundle")
		fixture.create(t, source)

		var stderr bytes.Buffer
		if code := run([]string{
			"--kind", fixture.kind,
			"--source", source,
			"--output", bundle,
		}, &stderr); code != 0 {
			t.Fatalf("export %s fixture: code=%d stderr=%q", fixture.kind, code, stderr.String())
		}
		data, err := os.ReadFile(bundle)
		if err != nil {
			t.Fatal(err)
		}
		fmt.Printf("%s\t%s\t%d\t%d\t%d\t%s\n",
			crossLanguageFixturePrefix,
			fixture.kind,
			fixture.sourceFormat,
			fixture.bucketCount,
			fixture.recordCount,
			hex.EncodeToString(data),
		)
	}
}

func createCrossLanguageProject(t *testing.T, path string) {
	t.Helper()
	var meta bytes.Buffer
	if err := gob.NewEncoder(&meta).Encode(model.Meta{
		Goal:          "cross-language migration fixture",
		Summary:       "preserve every opaque byte",
		FormatVersion: 5,
	}); err != nil {
		t.Fatal(err)
	}

	numericBuckets := []string{
		"plans",
		"tasks",
		"notes",
		"milestones",
		"issues",
		"commits",
		"capabilities",
		"capability_audits",
	}
	database, err := bolt.Open(path, 0o600, nil)
	if err != nil {
		t.Fatal(err)
	}
	if err := database.Update(func(transaction *bolt.Tx) error {
		metaBucket, err := transaction.CreateBucket([]byte("meta"))
		if err != nil {
			return err
		}
		if err := metaBucket.Put([]byte("meta"), meta.Bytes()); err != nil {
			return err
		}
		for index, name := range numericBuckets {
			bucket, err := transaction.CreateBucket([]byte(name))
			if err != nil {
				return err
			}
			identifier := uint64(index + 2)
			key := make([]byte, 8)
			binary.BigEndian.PutUint64(key, identifier)
			value := []byte{0x00, byte(index), 0xff, byte(len(name))}
			value = append(value, name...)
			if err := bucket.Put(key, value); err != nil {
				return err
			}
			if err := bucket.SetSequence(identifier + 100); err != nil {
				return err
			}
		}
		memory, err := transaction.CreateBucket([]byte("memory_writebacks"))
		if err != nil {
			return err
		}
		if err := memory.Put(
			[]byte("agent/run/receipt"),
			[]byte{0x00, 0xff, 'g', 'o', 'b', '-', 'r', 'e', 'c', 'e', 'i', 'p', 't'},
		); err != nil {
			return err
		}
		return memory.SetSequence(211)
	}); err != nil {
		_ = database.Close()
		t.Fatal(err)
	}
	if err := database.Close(); err != nil {
		t.Fatal(err)
	}
}

func createCrossLanguageGlobal(t *testing.T, path string) {
	t.Helper()
	database, err := bolt.Open(path, 0o600, nil)
	if err != nil {
		t.Fatal(err)
	}
	if err := database.Update(func(transaction *bolt.Tx) error {
		records := []struct {
			bucket string
			key    []byte
			value  []byte
		}{
			{bucket: "backups", key: []byte("2026-08-12T12:34:56Z"), value: []byte{0x00, 0xff, 'b', 'a', 'c', 'k', 'u', 'p'}},
			{bucket: "config", key: []byte("theme"), value: []byte("dark\x00mode")},
			{bucket: "projects", key: []byte("/fixture/project"), value: []byte{0xff, 0x00, 'g', 'o', 'b', '-', 'r', 'e', 'f'}},
		}
		for _, record := range records {
			bucket, err := transaction.CreateBucket([]byte(record.bucket))
			if err != nil {
				return err
			}
			if err := bucket.Put(record.key, record.value); err != nil {
				return err
			}
		}
		return nil
	}); err != nil {
		_ = database.Close()
		t.Fatal(err)
	}
	if err := database.Close(); err != nil {
		t.Fatal(err)
	}
}
