package capability

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"strings"
	"testing"
)

func TestMCPInitializeListAndCall(t *testing.T) {
	input := strings.Join([]string{
		`{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}`,
		`{"jsonrpc":"2.0","method":"notifications/initialized"}`,
		`{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}`,
		`{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"ptrack_http_request","arguments":{"capability_id":7,"request":{"method":"GET","url":"https://example.com"}}}}`,
	}, "\n") + "\n"
	var output bytes.Buffer
	var gotCall ToolCall
	err := ServeMCP(context.Background(), strings.NewReader(input), &output, func(_ context.Context, call ToolCall) (json.RawMessage, error) {
		gotCall = call
		return json.RawMessage(`{"status_code":200}`), nil
	})
	if err != nil {
		t.Fatal(err)
	}
	lines := strings.Split(strings.TrimSpace(output.String()), "\n")
	if len(lines) != 3 {
		t.Fatalf("responses=%d output=%s", len(lines), output.String())
	}
	if !strings.Contains(lines[0], `"protocolVersion":"2025-11-25"`) || !strings.Contains(lines[1], ToolHTTPRequest) {
		t.Fatalf("handshake/list output=%s", output.String())
	}
	if !strings.Contains(lines[2], `"structuredContent":{"status_code":200}`) || !strings.Contains(lines[2], `"type":"text"`) {
		t.Fatalf("call result=%s", lines[2])
	}
	if gotCall.Name != ToolHTTPRequest || !bytes.Contains(gotCall.Arguments, []byte(`"capability_id":7`)) {
		t.Fatalf("forwarded call=%+v", gotCall)
	}
}

func TestMCPReportsToolFailureInsideCallResult(t *testing.T) {
	input := `{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}` + "\n" +
		`{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"ptrack_git","arguments":{}}}` + "\n"
	var output bytes.Buffer
	err := ServeMCP(context.Background(), strings.NewReader(input), &output, func(context.Context, ToolCall) (json.RawMessage, error) {
		return nil, errors.New("capability denied")
	})
	if err != nil {
		t.Fatal(err)
	}
	lines := strings.Split(strings.TrimSpace(output.String()), "\n")
	if len(lines) != 2 || !strings.Contains(lines[0], `"protocolVersion":"2025-06-18"`) || !strings.Contains(lines[1], `"isError":true`) {
		t.Fatalf("output=%s", output.String())
	}
}
