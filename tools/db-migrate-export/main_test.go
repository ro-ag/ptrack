package main

import (
	"bytes"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	bolt "go.etcd.io/bbolt"
)

func TestRunRequiresOnlyExplicitFlags(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("exporter intentionally fails closed on Windows")
	}
	tests := []struct {
		name string
		args []string
		want string
	}{
		{name: "missing", args: nil, want: "required"},
		{name: "positional", args: []string{"--kind", "global", "--source", "/source", "--output", "/output", "extra"}, want: "positional"},
		{name: "invalid kind", args: []string{"--kind", "other", "--source", "/source", "--output", "/output"}, want: "project or global"},
		{name: "relative source", args: []string{"--kind", "global", "--source", "source.db", "--output", "/output"}, want: "absolute"},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			var stderr bytes.Buffer
			if code := run(test.args, &stderr); code == 0 {
				t.Fatal("run unexpectedly succeeded")
			}
			if !strings.Contains(stderr.String(), test.want) {
				t.Fatalf("stderr = %q, want substring %q", stderr.String(), test.want)
			}
		})
	}
}

func TestRunExportsExplicitGlobalDatabase(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("exporter intentionally fails closed on Windows")
	}
	dir := t.TempDir()
	source := filepath.Join(dir, "global.db")
	db, err := bolt.Open(source, 0o600, nil)
	if err != nil {
		t.Fatal(err)
	}
	if err := db.Update(func(tx *bolt.Tx) error {
		for _, name := range []string{"config", "projects", "backups"} {
			if _, err := tx.CreateBucket([]byte(name)); err != nil {
				return err
			}
		}
		return tx.Bucket([]byte("config")).Put([]byte("theme"), []byte("dark"))
	}); err != nil {
		_ = db.Close()
		t.Fatal(err)
	}
	if err := db.Close(); err != nil {
		t.Fatal(err)
	}
	output := filepath.Join(dir, "global.bundle")
	var stderr bytes.Buffer
	if code := run([]string{"--kind", "global", "--source", source, "--output", output}, &stderr); code != 0 {
		t.Fatalf("run code = %d, stderr = %q", code, stderr.String())
	}
	data, err := os.ReadFile(output)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.HasPrefix(data, []byte("PTRKMIG1")) {
		t.Fatalf("output prefix = %q", data[:8])
	}
}
