package terminal

import (
	"fmt"
	"io"
	"testing"

	"github.com/gorilla/websocket"
)

func TestStreamServerSustains100MiBWithAcknowledgements(t *testing.T) {
	const totalBytes = 100 * 1024 * 1024
	process := &stressSequenceProcess{
		managerFakeProcess: newManagerFakeProcess(),
		start:              make(chan struct{}),
		total:              totalBytes,
	}
	manager := newManagerForTest(t, t.TempDir(), newManagerFakeFactory(
		managerStartOutcome{process: process},
	))
	session, err := manager.Create("agent", "", 24, 80)
	if err != nil {
		t.Fatalf("Create: %v", err)
	}
	awaitSignal(t, process.readStarted, "PTY output read loop")
	awaitSignal(t, process.waitStarted, "PTY wait loop")
	cleanupManager(t, manager, process.managerFakeProcess)
	connection := dialStreamForTest(
		t,
		streamURLForTest(t, manager, session.ID()),
		"wails://wails",
	)
	defer connection.Close()
	close(process.start)

	received := 0
	for received < totalBytes {
		messageType, output, err := readStreamMessage(connection)
		if err != nil {
			t.Fatalf("read sustained output after %d bytes: %v", received, err)
		}
		if messageType != websocket.BinaryMessage || len(output) == 0 ||
			len(output) > testOutputChunkSize || received+len(output) > totalBytes {
			t.Fatalf("invalid sustained frame after %d bytes: type=%d size=%d", received, messageType, len(output))
		}
		for index, value := range output {
			if value != stressSequenceByte(received+index) {
				t.Fatalf("sustained output sequence mismatch at byte %d", received+index)
			}
		}
		received += len(output)
		if err := connection.WriteMessage(
			websocket.TextMessage,
			[]byte(fmt.Sprintf(`{"type":"ack","bytes":%d}`, len(output))),
		); err != nil {
			t.Fatalf("acknowledge sustained output after %d bytes: %v", received, err)
		}
	}
}

type stressSequenceProcess struct {
	*managerFakeProcess
	start  chan struct{}
	offset int
	total  int
}

func (p *stressSequenceProcess) Read(buffer []byte) (int, error) {
	p.readStartOnce.Do(func() { close(p.readStarted) })
	if p.offset == 0 {
		select {
		case <-p.start:
		case <-p.readClosed:
			return 0, io.EOF
		}
	}
	if p.offset >= p.total {
		<-p.readClosed
		return 0, io.EOF
	}
	count := min(len(buffer), testOutputChunkSize, p.total-p.offset)
	for index := 0; index < count; index++ {
		buffer[index] = stressSequenceByte(p.offset + index)
	}
	p.offset += count
	return count, nil
}

func stressSequenceByte(offset int) byte {
	chunk := offset / testOutputChunkSize
	withinChunk := offset % testOutputChunkSize
	if withinChunk < 8 {
		return byte(uint64(chunk) >> (8 * withinChunk))
	}
	return byte((chunk*131 + withinChunk*17) % 251)
}
