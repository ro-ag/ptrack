package agentrun

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"
)

func TestIntegrationServerRegisterHeartbeatExitAndCleanup(t *testing.T) {
	projectRoot := t.TempDir()
	globalHome := t.TempDir()
	registry := NewRegistry(Config{ProjectRoot: projectRoot})
	server, err := StartIntegrationServer(registry, IntegrationConfig{
		GlobalHome:  globalHome,
		ProjectRoot: projectRoot,
		Generation:  4,
	})
	if err != nil {
		t.Fatalf("StartIntegrationServer: %v", err)
	}
	t.Cleanup(func() {
		_ = server.Shutdown(context.Background())
		_ = registry.Shutdown(context.Background())
	})

	descriptorBytes, err := os.ReadFile(server.DescriptorPath())
	if err != nil {
		t.Fatalf("read descriptor: %v", err)
	}
	var descriptor IntegrationDescriptor
	if err := json.Unmarshal(descriptorBytes, &descriptor); err != nil {
		t.Fatalf("decode descriptor: %v", err)
	}
	if descriptor.ProjectRoot != projectRoot || descriptor.Generation != 4 ||
		!strings.HasPrefix(descriptor.URL, "http://127.0.0.1:") ||
		descriptor.RegistrationToken == "" {
		t.Fatalf("descriptor = %#v", descriptor)
	}
	if runtime.GOOS != "windows" {
		if info, statErr := os.Stat(server.DescriptorPath()); statErr != nil ||
			info.Mode().Perm() != 0o600 {
			t.Fatalf("descriptor permissions = %v err=%v, want 0600", info.Mode().Perm(), statErr)
		}
		if info, statErr := os.Stat(filepath.Dir(server.DescriptorPath())); statErr != nil ||
			info.Mode().Perm() != 0o700 {
			t.Fatalf("descriptor dir permissions = %v err=%v, want 0700", info.Mode().Perm(), statErr)
		}
	}

	registration := map[string]any{
		"profile":  "wrapper",
		"provider": "external-test",
		"pid":      8123,
		"cwd":      projectRoot,
	}
	unauthorized := integrationRequest(t, http.MethodPost, descriptor.URL+"/v1/runs/register", "wrong", registration)
	if unauthorized.StatusCode != http.StatusUnauthorized {
		t.Fatalf("unauthorized status = %d", unauthorized.StatusCode)
	}
	_ = unauthorized.Body.Close()

	response := integrationRequest(
		t,
		http.MethodPost,
		descriptor.URL+"/v1/runs/register",
		descriptor.RegistrationToken,
		registration,
	)
	if response.StatusCode != http.StatusCreated {
		body, _ := io.ReadAll(response.Body)
		t.Fatalf("register status = %d: %s", response.StatusCode, body)
	}
	var leaseResponse struct {
		ID         string `json:"id"`
		LeaseToken string `json:"leaseToken"`
	}
	if err := json.NewDecoder(response.Body).Decode(&leaseResponse); err != nil {
		t.Fatalf("decode registration: %v", err)
	}
	_ = response.Body.Close()
	if leaseResponse.ID == "" || leaseResponse.LeaseToken == "" {
		t.Fatalf("registration response = %#v", leaseResponse)
	}
	if run := registry.Snapshot(1)[0]; run.Association != nil {
		t.Fatalf("external registration self-associated run: %#v", run.Association)
	}
	if snapshotJSON, _ := json.Marshal(registry.Snapshot(10)); bytes.Contains(snapshotJSON, []byte(leaseResponse.LeaseToken)) {
		t.Fatal("lease token leaked into snapshot")
	}

	heartbeat := integrationRequest(
		t,
		http.MethodPost,
		descriptor.URL+"/v1/runs/"+leaseResponse.ID+"/heartbeat",
		leaseResponse.LeaseToken,
		nil,
	)
	if heartbeat.StatusCode != http.StatusNoContent {
		t.Fatalf("heartbeat status = %d", heartbeat.StatusCode)
	}
	_ = heartbeat.Body.Close()

	exit := integrationRequest(
		t,
		http.MethodPost,
		descriptor.URL+"/v1/runs/"+leaseResponse.ID+"/exit",
		leaseResponse.LeaseToken,
		map[string]any{"code": 0, "result": "done"},
	)
	if exit.StatusCode != http.StatusNoContent {
		t.Fatalf("exit status = %d", exit.StatusCode)
	}
	_ = exit.Body.Close()
	run := registry.Snapshot(1)[0]
	if run.State != StateExited || run.Exit == nil || run.Exit.Result != "done" {
		t.Fatalf("run after exit = %#v", run)
	}

	descriptorPath := server.DescriptorPath()
	if err := server.Shutdown(context.Background()); err != nil {
		t.Fatalf("Shutdown: %v", err)
	}
	if _, err := os.Stat(descriptorPath); !os.IsNotExist(err) {
		t.Fatalf("descriptor remains after shutdown: %v", err)
	}
}

func TestIntegrationServerRejectsExternalAssociationClaims(t *testing.T) {
	projectRoot := t.TempDir()
	registry := NewRegistry(Config{ProjectRoot: projectRoot})
	server, err := StartIntegrationServer(registry, IntegrationConfig{
		GlobalHome: t.TempDir(), ProjectRoot: projectRoot, Generation: 1,
	})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		_ = server.Shutdown(context.Background())
		_ = registry.Shutdown(context.Background())
	})
	contents, _ := os.ReadFile(server.DescriptorPath())
	var descriptor IntegrationDescriptor
	_ = json.Unmarshal(contents, &descriptor)
	response := integrationRequest(t, http.MethodPost, descriptor.URL+"/v1/runs/register",
		descriptor.RegistrationToken, map[string]any{
			"profile": "wrapper", "provider": "external", "cwd": projectRoot,
			"planId": 2, "taskId": 9,
		})
	defer response.Body.Close()
	if response.StatusCode != http.StatusBadRequest {
		t.Fatalf("association claim status = %d, want 400", response.StatusCode)
	}
	if len(registry.Snapshot(10)) != 0 {
		t.Fatal("rejected association claim registered a run")
	}
}

func TestOlderIntegrationServerCannotRemoveNewSameProjectDescriptor(t *testing.T) {
	projectRoot := t.TempDir()
	globalHome := t.TempDir()
	firstRegistry := NewRegistry(Config{ProjectRoot: projectRoot})
	secondRegistry := NewRegistry(Config{ProjectRoot: projectRoot})
	first, err := StartIntegrationServer(firstRegistry, IntegrationConfig{
		GlobalHome: globalHome, ProjectRoot: projectRoot, Generation: 1,
	})
	if err != nil {
		t.Fatalf("start first server: %v", err)
	}
	second, err := StartIntegrationServer(secondRegistry, IntegrationConfig{
		GlobalHome: globalHome, ProjectRoot: projectRoot, Generation: 2,
	})
	if err != nil {
		t.Fatalf("start replacement server: %v", err)
	}
	t.Cleanup(func() {
		_ = first.Shutdown(context.Background())
		_ = second.Shutdown(context.Background())
		_ = firstRegistry.Shutdown(context.Background())
		_ = secondRegistry.Shutdown(context.Background())
	})

	if err := first.Shutdown(context.Background()); err != nil {
		t.Fatalf("shutdown first server: %v", err)
	}
	descriptorBytes, err := os.ReadFile(second.DescriptorPath())
	if err != nil {
		t.Fatalf("replacement descriptor was removed: %v", err)
	}
	var descriptor IntegrationDescriptor
	if err := json.Unmarshal(descriptorBytes, &descriptor); err != nil {
		t.Fatalf("decode replacement descriptor: %v", err)
	}
	if descriptor.Generation != 2 {
		t.Fatalf("published generation = %d, want 2", descriptor.Generation)
	}

	if err := second.Shutdown(context.Background()); err != nil {
		t.Fatalf("shutdown second server: %v", err)
	}
	if _, err := os.Stat(second.DescriptorPath()); !os.IsNotExist(err) {
		t.Fatalf("owned descriptor remains after final shutdown: %v", err)
	}
}

func TestIntegrationServerRejectsBrowserAndOversizedRequests(t *testing.T) {
	projectRoot := t.TempDir()
	registry := NewRegistry(Config{ProjectRoot: projectRoot})
	server, err := StartIntegrationServer(registry, IntegrationConfig{
		GlobalHome: t.TempDir(), ProjectRoot: projectRoot, Generation: 1,
	})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		_ = server.Shutdown(context.Background())
		_ = registry.Shutdown(context.Background())
	})
	descriptorBytes, _ := os.ReadFile(server.DescriptorPath())
	var descriptor IntegrationDescriptor
	_ = json.Unmarshal(descriptorBytes, &descriptor)

	request, _ := http.NewRequest(
		http.MethodPost,
		descriptor.URL+"/v1/runs/register",
		strings.NewReader(`{"profile":"`+strings.Repeat("x", maxIntegrationBodyBytes)+`"}`),
	)
	request.Header.Set("Authorization", "Bearer "+descriptor.RegistrationToken)
	response, err := http.DefaultClient.Do(request)
	if err != nil {
		t.Fatal(err)
	}
	if response.StatusCode != http.StatusRequestEntityTooLarge {
		t.Fatalf("oversized status = %d", response.StatusCode)
	}
	_ = response.Body.Close()

	request, _ = http.NewRequest(http.MethodPost, descriptor.URL+"/v1/runs/register", strings.NewReader(`{}`))
	request.Header.Set("Authorization", "Bearer "+descriptor.RegistrationToken)
	request.Header.Set("Origin", "wails://wails")
	response, err = http.DefaultClient.Do(request)
	if err != nil {
		t.Fatal(err)
	}
	if response.StatusCode != http.StatusForbidden {
		t.Fatalf("browser-origin status = %d", response.StatusCode)
	}
	_ = response.Body.Close()
}

func integrationRequest(
	t *testing.T,
	method, uri, token string,
	body any,
) *http.Response {
	t.Helper()
	var reader io.Reader
	if body != nil {
		encoded, err := json.Marshal(body)
		if err != nil {
			t.Fatal(err)
		}
		reader = bytes.NewReader(encoded)
	}
	request, err := http.NewRequest(method, uri, reader)
	if err != nil {
		t.Fatal(err)
	}
	request.Header.Set("Authorization", "Bearer "+token)
	request.Header.Set("Content-Type", "application/json")
	client := &http.Client{Timeout: time.Second}
	response, err := client.Do(request)
	if err != nil {
		t.Fatal(err)
	}
	return response
}
