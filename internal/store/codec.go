package store

import (
	"bytes"
	"encoding/binary"
	"encoding/gob"
)

// gobEncode serializes v with encoding/gob.
func gobEncode(v any) ([]byte, error) {
	var buf bytes.Buffer
	if err := gob.NewEncoder(&buf).Encode(v); err != nil {
		return nil, err
	}
	return buf.Bytes(), nil
}

// gobDecode deserializes gob-encoded data into v (a pointer).
func gobDecode(data []byte, v any) error {
	return gob.NewDecoder(bytes.NewReader(data)).Decode(v)
}

// itob returns an 8-byte big-endian key for a uint64 id, so bbolt iterates ids
// in numeric order.
func itob(v uint64) []byte {
	b := make([]byte, 8)
	binary.BigEndian.PutUint64(b, v)
	return b
}
