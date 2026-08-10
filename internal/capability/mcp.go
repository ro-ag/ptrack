package capability

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"sync"
)

const (
	MCPProtocolVersion  = "2025-11-25"
	mcpPreviousProtocol = "2025-06-18"
	maxMCPMessageBytes  = maxBrokerBodyBytes
)

// MCPToolCaller forwards one validated MCP tool call.
type MCPToolCaller func(context.Context, ToolCall) (json.RawMessage, error)

// ServeMCP runs the newline-delimited JSON-RPC stdio bridge until EOF or
// context cancellation.
func ServeMCP(ctx context.Context, input io.Reader, output io.Writer, call MCPToolCaller) error {
	if call == nil {
		return errors.New("MCP tool caller is required")
	}
	scanner := bufio.NewScanner(input)
	scanner.Buffer(make([]byte, 64*1024), maxMCPMessageBytes)
	encoder := json.NewEncoder(output)
	var outputMu sync.Mutex
	initialized := false
	for scanner.Scan() {
		select {
		case <-ctx.Done():
			return ctx.Err()
		default:
		}
		line := bytes.TrimSpace(scanner.Bytes())
		if len(line) == 0 {
			continue
		}
		var request mcpRequest
		if err := json.Unmarshal(line, &request); err != nil || request.JSONRPC != "2.0" || request.Method == "" {
			if err := writeMCP(&outputMu, encoder, mcpResponse{JSONRPC: "2.0", Error: &mcpError{Code: -32700, Message: "parse error"}}); err != nil {
				return err
			}
			continue
		}
		response, notification := handleMCPRequest(ctx, request, &initialized, call)
		if notification {
			continue
		}
		if err := writeMCP(&outputMu, encoder, response); err != nil {
			return err
		}
	}
	if err := scanner.Err(); err != nil {
		return err
	}
	return nil
}

type mcpRequest struct {
	JSONRPC string          `json:"jsonrpc"`
	ID      json.RawMessage `json:"id,omitempty"`
	Method  string          `json:"method"`
	Params  json.RawMessage `json:"params,omitempty"`
}

type mcpResponse struct {
	JSONRPC string          `json:"jsonrpc"`
	ID      json.RawMessage `json:"id,omitempty"`
	Result  any             `json:"result,omitempty"`
	Error   *mcpError       `json:"error,omitempty"`
}

type mcpError struct {
	Code    int    `json:"code"`
	Message string `json:"message"`
}

func handleMCPRequest(ctx context.Context, request mcpRequest, initialized *bool, call MCPToolCaller) (mcpResponse, bool) {
	response := mcpResponse{JSONRPC: "2.0", ID: request.ID}
	isNotification := len(request.ID) == 0 || bytes.Equal(request.ID, []byte("null"))
	switch request.Method {
	case "initialize":
		if isNotification {
			return response, true
		}
		var params struct {
			ProtocolVersion string `json:"protocolVersion"`
		}
		if err := json.Unmarshal(request.Params, &params); err != nil {
			response.Error = &mcpError{Code: -32602, Message: "invalid initialize parameters"}
			return response, false
		}
		protocol := MCPProtocolVersion
		if params.ProtocolVersion == mcpPreviousProtocol {
			protocol = mcpPreviousProtocol
		}
		*initialized = true
		response.Result = map[string]any{
			"protocolVersion": protocol,
			"capabilities":    map[string]any{"tools": map[string]any{"listChanged": false}},
			"serverInfo":      map[string]any{"name": "p-track-capabilities", "version": "1"},
		}
	case "notifications/initialized":
		return response, true
	case "ping":
		response.Result = map[string]any{}
	case "tools/list":
		if !*initialized {
			response.Error = &mcpError{Code: -32002, Message: "server is not initialized"}
			return response, isNotification
		}
		response.Result = map[string]any{"tools": ToolDefinitions()}
	case "tools/call":
		if !*initialized {
			response.Error = &mcpError{Code: -32002, Message: "server is not initialized"}
			return response, isNotification
		}
		var params struct {
			Name      string         `json:"name"`
			Arguments map[string]any `json:"arguments"`
		}
		if err := json.Unmarshal(request.Params, &params); err != nil || !knownTool(params.Name) {
			response.Error = &mcpError{Code: -32602, Message: "unknown tool or invalid parameters"}
			return response, isNotification
		}
		arguments, err := json.Marshal(params.Arguments)
		if err != nil {
			response.Error = &mcpError{Code: -32602, Message: "invalid tool arguments"}
			return response, isNotification
		}
		result, err := call(ctx, ToolCall{Name: params.Name, Arguments: arguments})
		if err != nil {
			response.Result = map[string]any{
				"content": []map[string]any{{"type": "text", "text": err.Error()}},
				"isError": true,
			}
			return response, isNotification
		}
		var structured map[string]any
		if err := json.Unmarshal(result, &structured); err != nil {
			response.Result = map[string]any{
				"content": []map[string]any{{"type": "text", "text": string(result)}},
				"isError": false,
			}
			return response, isNotification
		}
		response.Result = map[string]any{
			"content":           []map[string]any{{"type": "text", "text": string(result)}},
			"structuredContent": structured,
			"isError":           false,
		}
	default:
		response.Error = &mcpError{Code: -32601, Message: "method not found"}
	}
	return response, isNotification
}

func knownTool(name string) bool {
	for _, tool := range ToolDefinitions() {
		if tool.Name == name {
			return true
		}
	}
	return false
}

func writeMCP(mu *sync.Mutex, encoder *json.Encoder, response mcpResponse) error {
	mu.Lock()
	defer mu.Unlock()
	if err := encoder.Encode(response); err != nil {
		return fmt.Errorf("write MCP response: %w", err)
	}
	return nil
}
