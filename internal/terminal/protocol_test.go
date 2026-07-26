package terminal

import (
	"bytes"
	"reflect"
	"sync"
	"sync/atomic"
	"testing"
)

func TestParseACKControlAcceptsOnlyStrictPositiveIntegerACK(t *testing.T) {
	tests := [][]byte{
		[]byte(`{"type":"ack","bytes":1}`),
		[]byte(`{"bytes":65536,"type":"ack"}`),
		[]byte(" \n\t" + `{"type":"ack","bytes":42}` + "\r\n"),
	}
	for _, payload := range tests {
		got, err := parseACKControl(payload)
		if err != nil {
			t.Fatalf("parseACKControl(%q): %v", payload, err)
		}
		if got <= 0 {
			t.Fatalf("parseACKControl(%q) = %d, want positive byte count", payload, got)
		}
	}
}

func TestParseACKControlRejectsMalformedOrNonStrictFrames(t *testing.T) {
	tests := []struct {
		name    string
		payload []byte
	}{
		{name: "empty", payload: nil},
		{name: "empty object", payload: []byte(`{}`)},
		{name: "missing type", payload: []byte(`{"bytes":1}`)},
		{name: "missing bytes", payload: []byte(`{"type":"ack"}`)},
		{name: "unknown type", payload: []byte(`{"type":"credit","bytes":1}`)},
		{name: "zero", payload: []byte(`{"type":"ack","bytes":0}`)},
		{name: "negative", payload: []byte(`{"type":"ack","bytes":-1}`)},
		{name: "fractional", payload: []byte(`{"type":"ack","bytes":1.5}`)},
		{name: "exponent notation", payload: []byte(`{"type":"ack","bytes":1e2}`)},
		{name: "string bytes", payload: []byte(`{"type":"ack","bytes":"1"}`)},
		{name: "null bytes", payload: []byte(`{"type":"ack","bytes":null}`)},
		{name: "unknown field", payload: []byte(`{"type":"ack","bytes":1,"session":"secret"}`)},
		{name: "duplicate type", payload: []byte(`{"type":"ack","type":"ack","bytes":1}`)},
		{name: "duplicate bytes", payload: []byte(`{"type":"ack","bytes":1,"bytes":1}`)},
		{name: "trailing object", payload: []byte(`{"type":"ack","bytes":1}{}`)},
		{name: "trailing scalar", payload: []byte(`{"type":"ack","bytes":1} true`)},
		{name: "array", payload: []byte(`[{"type":"ack","bytes":1}]`)},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			if got, err := parseACKControl(test.payload); err == nil {
				t.Fatalf("parseACKControl(%q) = %d, want error", test.payload, got)
			}
		})
	}
}

func TestParseACKControlRejectsOversizedFrame(t *testing.T) {
	payload := append([]byte(`{"type":"ack","bytes":1}`), bytes.Repeat([]byte(" "), maxControlFrameBytes)...)
	if len(payload) <= maxControlFrameBytes {
		t.Fatalf("test frame length = %d, want over limit %d", len(payload), maxControlFrameBytes)
	}
	if got, err := parseACKControl(payload); err == nil {
		t.Fatalf("parseACKControl oversized frame = %d, want error", got)
	}
}

func TestFlowLedgerRejectsACKBeyondBytesActuallySent(t *testing.T) {
	ledger := newFlowLedger(outputWindowBytes)
	if !ledger.tryReserve(outputChunkBytes) {
		t.Fatal("failed to reserve first output chunk")
	}
	before := ledger.unacknowledged()

	if err := ledger.acknowledge(outputChunkBytes + 1); err == nil {
		t.Fatal("ACK beyond bytes actually sent succeeded")
	}
	if got := ledger.unacknowledged(); got != before {
		t.Fatalf("failed ACK changed unacknowledged bytes from %d to %d", before, got)
	}
	if err := ledger.acknowledge(0); err == nil {
		t.Fatal("zero-byte ACK succeeded")
	}
	if err := ledger.acknowledge(-1); err == nil {
		t.Fatal("negative-byte ACK succeeded")
	}
}

func TestFlowLedgerLeavesRoomForOneInflightMaximumChunkAndResumesAfterACK(t *testing.T) {
	ledger := newFlowLedger(outputWindowBytes)
	maxReserved := outputWindowBytes - outputChunkBytes
	reservations := maxReserved / outputChunkBytes

	for index := 0; index < reservations; index++ {
		if !ledger.tryReserve(outputChunkBytes) {
			t.Fatalf("reservation %d of %d failed", index+1, reservations)
		}
	}
	if got := ledger.unacknowledged(); got != maxReserved {
		t.Fatalf("unacknowledged bytes = %d, want %d", got, maxReserved)
	}
	if got := ledger.unacknowledged() + outputChunkBytes; got > outputWindowBytes {
		t.Fatalf("reserved plus in-flight bytes = %d, exceeds window %d", got, outputWindowBytes)
	}
	if ledger.tryReserve(1) {
		t.Fatal("reservation succeeded without room for an in-flight maximum chunk")
	}
	if ledger.tryReserve(outputChunkBytes + 1) {
		t.Fatal("reservation larger than the maximum output chunk succeeded")
	}

	if err := ledger.acknowledge(outputChunkBytes); err != nil {
		t.Fatalf("acknowledge: %v", err)
	}
	if !ledger.tryReserve(outputChunkBytes) {
		t.Fatal("capacity did not resume after ACK")
	}
	if got := ledger.unacknowledged(); got != maxReserved {
		t.Fatalf("unacknowledged bytes after ACK and reserve = %d, want %d", got, maxReserved)
	}
}

func TestFlowLedgerConcurrentReservationsRemainWithinBound(t *testing.T) {
	ledger := newFlowLedger(outputWindowBytes)
	const goroutines = 128
	start := make(chan struct{})
	var successful atomic.Int64
	var waitGroup sync.WaitGroup
	waitGroup.Add(goroutines)

	for index := 0; index < goroutines; index++ {
		go func() {
			defer waitGroup.Done()
			<-start
			if ledger.tryReserve(outputChunkBytes) {
				successful.Add(1)
			}
		}()
	}
	close(start)
	waitGroup.Wait()

	maxReserved := outputWindowBytes - outputChunkBytes
	if got := ledger.unacknowledged(); got > maxReserved {
		t.Fatalf("concurrent reservations = %d bytes, exceed bound %d", got, maxReserved)
	}
	wantSuccessful := int64(maxReserved / outputChunkBytes)
	if got := successful.Load(); got != wantSuccessful {
		t.Fatalf("successful concurrent reservations = %d, want %d", got, wantSuccessful)
	}
	if got := ledger.unacknowledged() + outputChunkBytes; got > outputWindowBytes {
		t.Fatalf("reserved plus one in-flight chunk = %d, exceeds %d", got, outputWindowBytes)
	}
}

func TestSplitOutputCapsChunksAt64KiB(t *testing.T) {
	input := make([]byte, 2*outputChunkBytes+17)
	for index := range input {
		input[index] = byte(index)
	}

	chunks := splitOutput(input)
	wantLengths := []int{outputChunkBytes, outputChunkBytes, 17}
	gotLengths := make([]int, 0, len(chunks))
	var reassembled []byte
	for _, chunk := range chunks {
		gotLengths = append(gotLengths, len(chunk))
		if len(chunk) == 0 || len(chunk) > outputChunkBytes {
			t.Fatalf("chunk length = %d, want 1..%d", len(chunk), outputChunkBytes)
		}
		reassembled = append(reassembled, chunk...)
	}
	if !reflect.DeepEqual(gotLengths, wantLengths) {
		t.Fatalf("chunk lengths = %#v, want %#v", gotLengths, wantLengths)
	}
	if !bytes.Equal(reassembled, input) {
		t.Fatal("split chunks do not reassemble to input")
	}

	if got := splitOutput(nil); len(got) != 0 {
		t.Fatalf("splitOutput(nil) = %#v, want no chunks", got)
	}
	if got := splitOutput(make([]byte, outputChunkBytes)); len(got) != 1 || len(got[0]) != outputChunkBytes {
		t.Fatalf("exact-size split = %#v, want one %d-byte chunk", chunkLengths(got), outputChunkBytes)
	}
}

func chunkLengths(chunks [][]byte) []int {
	lengths := make([]int, len(chunks))
	for index, chunk := range chunks {
		lengths[index] = len(chunk)
	}
	return lengths
}
