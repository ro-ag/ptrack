package store

import (
	"sort"

	"github.com/ro-ag/ptrack/internal/model"
	bolt "go.etcd.io/bbolt"
)

// putGob gob-encodes v and stores it under key in bucket b.
func putGob(b *bolt.Bucket, key []byte, v any) error {
	data, err := gobEncode(v)
	if err != nil {
		return err
	}
	return b.Put(key, data)
}

// getGob decodes the value at key into v. Missing keys decode as the zero value
// (no error); use getGobNF when absence should be ErrNotFound.
func getGob(b *bolt.Bucket, key []byte, v any) error {
	data := b.Get(key)
	if data == nil {
		return nil
	}
	return gobDecode(data, v)
}

// getGobNF decodes the value at key into v, returning ErrNotFound if absent.
func getGobNF(b *bolt.Bucket, key []byte, v any) error {
	data := b.Get(key)
	if data == nil {
		return ErrNotFound
	}
	return gobDecode(data, v)
}

type ordered interface{ Ord() int }

// sortByOrder sorts xs in place by ascending Order.
func sortByOrder[T ordered](xs []T) {
	sort.SliceStable(xs, func(i, j int) bool { return xs[i].Ord() < xs[j].Ord() })
}

// sortByLastSeenDesc sorts project refs most-recently-seen first.
func sortByLastSeenDesc(refs []model.ProjectRef) {
	sort.SliceStable(refs, func(i, j int) bool { return refs[i].LastSeen.After(refs[j].LastSeen) })
}
