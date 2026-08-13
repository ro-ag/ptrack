package capability

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"flag"
	"os"
	"path/filepath"
	"testing"

	"github.com/ro-ag/ptrack/internal/model"
)

var updateDigestFixtures = flag.Bool("update-digest-fixtures", false, "rewrite canonical capability digest fixtures")

type canonicalDigestFixtures struct {
	Fixtures []canonicalDigestFixture `json:"fixtures"`
}

type canonicalDigestFixture struct {
	Name          string           `json:"name"`
	Draft         model.Capability `json:"draft"`
	CanonicalJSON string           `json:"canonical_json"`
	Digest        string           `json:"digest"`
}

func TestCanonicalDigestFixtures(t *testing.T) {
	path := filepath.Join("testdata", "canonical_digest_fixtures.json")
	encoded, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var fixtures canonicalDigestFixtures
	if err := json.Unmarshal(encoded, &fixtures); err != nil {
		t.Fatal(err)
	}
	for index := range fixtures.Fixtures {
		fixture := &fixtures.Fixtures[index]
		draftJSON, err := json.Marshal(fixture.Draft)
		if err != nil {
			t.Fatalf("%s: %v", fixture.Name, err)
		}
		var draft model.Capability
		if err := json.Unmarshal(draftJSON, &draft); err != nil {
			t.Fatalf("%s: %v", fixture.Name, err)
		}
		preview, err := Normalize(draft)
		if err != nil {
			t.Fatalf("%s: %v", fixture.Name, err)
		}
		canonical, err := canonicalDigestJSON(preview.Capability)
		if err != nil {
			t.Fatalf("%s: %v", fixture.Name, err)
		}
		digest := sha256.Sum256(canonical)
		actualDigest := hex.EncodeToString(digest[:])
		if *updateDigestFixtures {
			fixture.CanonicalJSON = string(canonical)
			fixture.Digest = actualDigest
			continue
		}
		if fixture.CanonicalJSON != string(canonical) {
			t.Errorf("%s canonical JSON mismatch\nwant %s\n got %s", fixture.Name, fixture.CanonicalJSON, canonical)
		}
		if fixture.Digest != actualDigest || preview.ScopeDigest != actualDigest {
			t.Errorf("%s digest: fixture=%s preview=%s actual=%s", fixture.Name, fixture.Digest, preview.ScopeDigest, actualDigest)
		}
	}
	if *updateDigestFixtures {
		encoded, err := json.MarshalIndent(fixtures, "", "  ")
		if err != nil {
			t.Fatal(err)
		}
		encoded = append(encoded, '\n')
		if err := os.WriteFile(path, encoded, 0o644); err != nil {
			t.Fatal(err)
		}
	}
}

func canonicalDigestJSON(capability model.Capability) ([]byte, error) {
	envelope := struct {
		ModelVersion            uint
		Kind                    model.CapabilityKind
		AgentProfile            string
		ApprovalDurationSeconds int64
		Limits                  model.CapabilityLimits
		Audit                   model.CapabilityAuditPolicy
		HTTP                    *model.HTTPScope
		Git                     *model.GitScope
		SSH                     *model.SSHScope
	}{
		capability.ModelVersion, capability.Kind, capability.AgentProfile,
		capability.ApprovalDurationSeconds, capability.Limits, capability.Audit,
		capability.HTTP, capability.Git, capability.SSH,
	}
	return json.Marshal(envelope)
}
