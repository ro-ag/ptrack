package terminal

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"strconv"
	"sync"
)

const (
	outputChunkBytes     = 64 * 1024
	outputWindowBytes    = 512 * 1024
	maxControlFrameBytes = 1024
	maxInputFrameBytes   = 64 * 1024
)

func parseACKControl(payload []byte) (int, error) {
	if len(payload) == 0 || len(payload) > maxControlFrameBytes {
		return 0, errors.New("invalid ACK control frame size")
	}

	decoder := json.NewDecoder(bytes.NewReader(payload))
	token, err := decoder.Token()
	if err != nil || token != json.Delim('{') {
		return 0, errors.New("ACK control frame must be an object")
	}

	var (
		controlType string
		byteCount   int
		seenType    bool
		seenBytes   bool
	)
	for decoder.More() {
		keyToken, keyErr := decoder.Token()
		if keyErr != nil {
			return 0, errors.New("invalid ACK control field")
		}
		key, ok := keyToken.(string)
		if !ok {
			return 0, errors.New("invalid ACK control field name")
		}
		switch key {
		case "type":
			if seenType {
				return 0, errors.New("duplicate ACK control type")
			}
			seenType = true
			if err := decoder.Decode(&controlType); err != nil {
				return 0, errors.New("ACK control type must be a string")
			}
		case "bytes":
			if seenBytes {
				return 0, errors.New("duplicate ACK control byte count")
			}
			seenBytes = true
			var raw json.RawMessage
			if err := decoder.Decode(&raw); err != nil {
				return 0, errors.New("invalid ACK control byte count")
			}
			if len(raw) == 0 || raw[0] < '1' || raw[0] > '9' {
				return 0, errors.New("ACK byte count must be a positive integer")
			}
			for _, digit := range raw[1:] {
				if digit < '0' || digit > '9' {
					return 0, errors.New("ACK byte count must be a positive integer")
				}
			}
			byteCount, err = strconv.Atoi(string(raw))
			if err != nil || byteCount <= 0 {
				return 0, errors.New("ACK byte count is out of range")
			}
		default:
			return 0, fmt.Errorf("unknown ACK control field %q", key)
		}
	}
	if _, err := decoder.Token(); err != nil {
		return 0, errors.New("invalid ACK control object")
	}
	if !seenType || !seenBytes || controlType != "ack" {
		return 0, errors.New("invalid ACK control frame")
	}
	if token, err := decoder.Token(); err != io.EOF || token != nil {
		return 0, errors.New("trailing ACK control data")
	}
	return byteCount, nil
}

type flowLedger struct {
	mu       sync.Mutex
	window   int
	reserved int
	sent     int
	changed  chan struct{}
}

func newFlowLedger(window int) *flowLedger {
	return &flowLedger{
		window:  window,
		changed: make(chan struct{}, 1),
	}
}

func (l *flowLedger) tryReserve(byteCount int) bool {
	if !l.tryReservePending(byteCount) {
		return false
	}
	l.markSent(byteCount)
	return true
}

func (l *flowLedger) tryReservePending(byteCount int) bool {
	if byteCount <= 0 || byteCount > outputChunkBytes {
		return false
	}
	l.mu.Lock()
	defer l.mu.Unlock()
	maxReserved := l.window - outputChunkBytes
	if maxReserved < 0 || l.reserved > maxReserved-byteCount {
		return false
	}
	l.reserved += byteCount
	return true
}

func (l *flowLedger) markSent(byteCount int) {
	l.mu.Lock()
	l.sent += byteCount
	l.mu.Unlock()
}

func (l *flowLedger) commit(byteCount int, send func() error) error {
	l.mu.Lock()
	defer l.mu.Unlock()
	if err := send(); err != nil {
		return err
	}
	l.sent += byteCount
	return nil
}

func (l *flowLedger) acknowledge(byteCount int) error {
	if byteCount <= 0 {
		return errors.New("ACK byte count must be positive")
	}
	l.mu.Lock()
	defer l.mu.Unlock()
	if byteCount > l.sent {
		return errors.New("ACK exceeds bytes sent")
	}
	l.reserved -= byteCount
	l.sent -= byteCount
	select {
	case l.changed <- struct{}{}:
	default:
	}
	return nil
}

func (l *flowLedger) release(byteCount int) {
	if byteCount <= 0 {
		return
	}
	l.mu.Lock()
	l.reserved = max(0, l.reserved-byteCount)
	l.mu.Unlock()
	select {
	case l.changed <- struct{}{}:
	default:
	}
}

func (l *flowLedger) unacknowledged() int {
	l.mu.Lock()
	defer l.mu.Unlock()
	return l.reserved
}

func (l *flowLedger) reservePending(ctx context.Context, byteCount int) error {
	for {
		if l.tryReservePending(byteCount) {
			return nil
		}
		select {
		case <-l.changed:
		case <-ctx.Done():
			return ctx.Err()
		}
	}
}

func splitOutput(output []byte) [][]byte {
	if len(output) == 0 {
		return nil
	}
	chunks := make([][]byte, 0, (len(output)+outputChunkBytes-1)/outputChunkBytes)
	for len(output) > 0 {
		size := min(len(output), outputChunkBytes)
		chunks = append(chunks, output[:size])
		output = output[size:]
	}
	return chunks
}
