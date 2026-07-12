package store

import (
	"io"
	"os"
	"path/filepath"
	"strconv"
	"time"

	"github.com/ro-ag/ptrack/internal/model"
	bolt "go.etcd.io/bbolt"
)

var (
	bucketConfig   = []byte("config")
	bucketProjects = []byte("projects")
	bucketBackups  = []byte("backups")
)

// Global is a handle to the global bbolt database (~/.ptrack/global.db, or
// $PTRACK_HOME/global.db when PTRACK_HOME is set).
type Global struct {
	db *bolt.DB
}

// GlobalHome returns the global ptrack directory: $PTRACK_HOME if set, else
// ~/.ptrack.
func GlobalHome() (string, error) {
	if h := os.Getenv("PTRACK_HOME"); h != "" {
		return h, nil
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(home, ".ptrack"), nil
}

// OpenGlobal opens (creating if needed) the global database and its buckets.
func OpenGlobal() (*Global, error) {
	home, err := GlobalHome()
	if err != nil {
		return nil, err
	}
	if err := os.MkdirAll(home, 0o755); err != nil {
		return nil, err
	}
	db, err := bolt.Open(filepath.Join(home, "global.db"), 0o600, &bolt.Options{Timeout: time.Second})
	if err != nil {
		return nil, err
	}
	g := &Global{db: db}
	err = db.Update(func(tx *bolt.Tx) error {
		for _, b := range [][]byte{bucketConfig, bucketProjects, bucketBackups} {
			if _, err := tx.CreateBucketIfNotExists(b); err != nil {
				return err
			}
		}
		return nil
	})
	if err != nil {
		_ = db.Close()
		return nil, err
	}
	return g, nil
}

// Close closes the global database.
func (g *Global) Close() error { return g.db.Close() }

// SetConfig stores a config key/value pair.
func (g *Global) SetConfig(key, val string) error {
	return g.db.Update(func(tx *bolt.Tx) error {
		return tx.Bucket(bucketConfig).Put([]byte(key), []byte(val))
	})
}

// GetConfig returns the value for key, or "" if unset.
func (g *Global) GetConfig(key string) (string, error) {
	var val string
	err := g.db.View(func(tx *bolt.Tx) error {
		if v := tx.Bucket(bucketConfig).Get([]byte(key)); v != nil {
			val = string(v)
		}
		return nil
	})
	return val, err
}

// RegisterProject records or refreshes a project in the registry, keyed by
// absolute path, updating LastSeen.
func (g *Global) RegisterProject(name, path string) error {
	abs, err := filepath.Abs(path)
	if err != nil {
		return err
	}
	return g.db.Update(func(tx *bolt.Tx) error {
		return putGob(tx.Bucket(bucketProjects), []byte(abs), model.ProjectRef{
			Name:     name,
			Path:     abs,
			LastSeen: time.Now(),
		})
	})
}

// ListProjects returns all registered projects, most-recently-seen first.
func (g *Global) ListProjects() ([]model.ProjectRef, error) {
	var refs []model.ProjectRef
	err := g.db.View(func(tx *bolt.Tx) error {
		return tx.Bucket(bucketProjects).ForEach(func(_, v []byte) error {
			var r model.ProjectRef
			if err := gobDecode(v, &r); err != nil {
				return err
			}
			refs = append(refs, r)
			return nil
		})
	})
	if err != nil {
		return nil, err
	}
	sortByLastSeenDesc(refs)
	return refs, nil
}

// RecordBackup notes that projectPath was backed up to backupPath.
func (g *Global) RecordBackup(projectPath, backupPath string) error {
	return g.db.Update(func(tx *bolt.Tx) error {
		key := []byte(strconv.FormatInt(time.Now().UnixNano(), 10))
		return tx.Bucket(bucketBackups).Put(key, []byte(projectPath+"\t"+backupPath))
	})
}

// BackupProject copies the project DB at projectDBPath into destDir as
// <projectDirName>-<ts>.db and returns the backup path. ts is supplied by the
// caller (typically time.Now().Unix()) so the operation is deterministic in
// tests.
func BackupProject(projectDBPath, destDir string, ts int64) (string, error) {
	if err := os.MkdirAll(destDir, 0o755); err != nil {
		return "", err
	}
	// name after the project directory (parent of .ptrack), not "ptrack".
	projRoot := filepath.Dir(filepath.Dir(projectDBPath))
	name := filepath.Base(projRoot)
	dst := filepath.Join(destDir, name+"-"+strconv.FormatInt(ts, 10)+".db")
	if err := copyFile(projectDBPath, dst); err != nil {
		return "", err
	}
	return dst, nil
}

func copyFile(src, dst string) error {
	in, err := os.Open(src)
	if err != nil {
		return err
	}
	defer in.Close()
	out, err := os.Create(dst)
	if err != nil {
		return err
	}
	if _, err := io.Copy(out, in); err != nil {
		_ = out.Close()
		return err
	}
	return out.Close()
}
