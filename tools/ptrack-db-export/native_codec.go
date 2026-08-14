package main

import (
	"bytes"
	"encoding/binary"
	"encoding/gob"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"math"
	"path/filepath"
	"strconv"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/ro-ag/ptrack/internal/model"
)

const (
	nativeRecordCodec   uint16 = 3
	nativePayloadSchema uint32 = 1
	legacyRawCodec      uint16 = 2
	nativeMaxPayload           = 256 << 20
	nativeMaxListItems         = 1_000_000
)

type nativeRecordKind uint16

const (
	nativeKindMeta nativeRecordKind = iota + 1
	nativeKindPlan
	nativeKindTask
	nativeKindNote
	nativeKindMilestone
	nativeKindIssue
	nativeKindCommit
	nativeKindCapability
	nativeKindCapabilityAudit
	nativeKindMemoryWriteback
	nativeKindProjectRef
	nativeKindGlobalConfig
	nativeKindGlobalBackup
)

type nativeRecordEncoding struct {
	Kind          nativeRecordKind
	Codec         uint16
	PayloadSchema uint32
	Payload       []byte
}

// encodeLegacyRecord strictly decodes one legacy record and translates it to
// the dependency-free canonical native payload. The caller supplies the
// authoritative collection and key; both are validated against the value.
// Raw global config and backup records retain the legacy raw codec after their
// structure has been validated.
func encodeLegacyRecord(collection string, key, raw []byte) (nativeRecordEncoding, error) {
	var (
		kind  nativeRecordKind
		value any
	)
	switch collection {
	case "meta":
		kind, value = nativeKindMeta, &model.Meta{}
	case "plans":
		kind, value = nativeKindPlan, &model.Plan{}
	case "tasks":
		kind, value = nativeKindTask, &model.Task{}
	case "notes":
		kind, value = nativeKindNote, &model.Note{}
	case "milestones":
		kind, value = nativeKindMilestone, &model.Milestone{}
	case "issues":
		kind, value = nativeKindIssue, &model.Issue{}
	case "commits":
		kind, value = nativeKindCommit, &model.Commit{}
	case "capabilities":
		kind, value = nativeKindCapability, &model.Capability{}
	case "capability_audits":
		kind, value = nativeKindCapabilityAudit, &model.CapabilityAudit{}
	case "memory_writebacks":
		kind, value = nativeKindMemoryWriteback, &memoryWritebackRecord{}
	case "projects":
		kind, value = nativeKindProjectRef, &model.ProjectRef{}
	case "config":
		if err := validateGlobalConfig(key, raw); err != nil {
			return nativeRecordEncoding{}, recordError(collection, key, err)
		}
		return rawNativeEncoding(nativeKindGlobalConfig, raw), nil
	case "backups":
		if err := validateGlobalBackup(key, raw); err != nil {
			return nativeRecordEncoding{}, recordError(collection, key, err)
		}
		return rawNativeEncoding(nativeKindGlobalBackup, raw), nil
	default:
		return nativeRecordEncoding{}, fmt.Errorf("unsupported legacy collection %q", collection)
	}

	if err := strictGobDecode(raw, value); err != nil {
		return nativeRecordEncoding{}, recordError(collection, key, err)
	}
	encoder := nativeEncoder{}
	if err := encoder.record(kind, key, value); err != nil {
		return nativeRecordEncoding{}, recordError(collection, key, err)
	}
	return nativeRecordEncoding{
		Kind:          kind,
		Codec:         nativeRecordCodec,
		PayloadSchema: nativePayloadSchema,
		Payload:       encoder.bytes(),
	}, nil
}

func rawNativeEncoding(kind nativeRecordKind, raw []byte) nativeRecordEncoding {
	return nativeRecordEncoding{
		Kind:          kind,
		Codec:         legacyRawCodec,
		PayloadSchema: 0,
		Payload:       append([]byte(nil), raw...),
	}
}

func strictGobDecode(raw []byte, destination any) error {
	reader := strictGobReader{data: raw}
	decoder := gob.NewDecoder(&reader)
	if err := decoder.Decode(destination); err != nil {
		return fmt.Errorf("decode legacy gob: %w", err)
	}
	if reader.offset != len(raw) {
		return fmt.Errorf("legacy gob has %d trailing bytes", len(raw)-reader.offset)
	}
	return nil
}

// strictGobReader implements io.ByteReader so gob.Decoder does not wrap it in a
// read-ahead buffer. Message bodies can still be copied in normal sized reads,
// while the consumed offset remains an exact end-of-value boundary.
type strictGobReader struct {
	data   []byte
	offset int
}

func (r *strictGobReader) Read(destination []byte) (int, error) {
	if r.offset == len(r.data) {
		return 0, io.EOF
	}
	count := copy(destination, r.data[r.offset:])
	r.offset += count
	return count, nil
}

func (r *strictGobReader) ReadByte() (byte, error) {
	if r.offset == len(r.data) {
		return 0, io.EOF
	}
	value := r.data[r.offset]
	r.offset++
	return value, nil
}

func recordError(collection string, key []byte, err error) error {
	return fmt.Errorf("invalid legacy %s record with key %x: %w", collection, key, err)
}

type nativeEncoder struct {
	buffer bytes.Buffer
	err    error
}

func (e *nativeEncoder) bytes() []byte {
	return append([]byte(nil), e.buffer.Bytes()...)
}

func (e *nativeEncoder) record(kind nativeRecordKind, key []byte, value any) error {
	switch kind {
	case nativeKindMeta:
		return e.meta(key, value.(*model.Meta))
	case nativeKindPlan:
		return e.plan(key, value.(*model.Plan))
	case nativeKindTask:
		return e.task(key, value.(*model.Task))
	case nativeKindNote:
		return e.note(key, value.(*model.Note))
	case nativeKindMilestone:
		return e.milestone(key, value.(*model.Milestone))
	case nativeKindIssue:
		return e.issue(key, value.(*model.Issue))
	case nativeKindCommit:
		return e.commit(key, value.(*model.Commit))
	case nativeKindCapability:
		return e.capability(key, value.(*model.Capability))
	case nativeKindCapabilityAudit:
		return e.capabilityAudit(key, value.(*model.CapabilityAudit))
	case nativeKindMemoryWriteback:
		return e.memoryWriteback(key, value.(*memoryWritebackRecord))
	case nativeKindProjectRef:
		return e.projectRef(key, value.(*model.ProjectRef))
	default:
		return fmt.Errorf("unsupported native record kind %d", kind)
	}
}

func (e *nativeEncoder) meta(key []byte, value *model.Meta) error {
	if !bytes.Equal(key, keyMeta) {
		return errors.New("meta key must be exactly meta")
	}
	if value.FormatVersion > CurrentFormat {
		return fmt.Errorf("format version %d exceeds supported version %d", value.FormatVersion, CurrentFormat)
	}
	e.string(value.Goal)
	e.string(value.Summary)
	e.u64(value.ActivePlan)
	e.timestamp(value.CreatedAt)
	e.timestamp(value.UpdatedAt)
	e.u64(uint64(value.FormatVersion))
	e.string(value.LastWriteVersion)
	return e.err
}

func (e *nativeEncoder) plan(key []byte, value *model.Plan) error {
	if err := validateRecordID(key, value.ID); err != nil {
		return err
	}
	status, err := planStatusTag(value.Status)
	if err != nil {
		return err
	}
	if value.Order < 0 {
		return errors.New("plan order must be nonnegative")
	}
	e.u64(value.ID)
	e.string(value.Title)
	e.u8(status)
	e.u64(value.MilestoneID)
	e.integer(value.Order)
	e.timestamp(value.CreatedAt)
	e.timestamp(value.UpdatedAt)
	return e.err
}

func (e *nativeEncoder) task(key []byte, value *model.Task) error {
	if err := validateRecordID(key, value.ID); err != nil {
		return err
	}
	status, err := taskStatusTag(value.Status)
	if err != nil {
		return err
	}
	if value.PlanID == 0 {
		return errors.New("task plan ID must be nonzero")
	}
	if value.Order < 0 {
		return errors.New("task order must be nonnegative")
	}
	e.u64(value.ID)
	e.u64(value.PlanID)
	e.string(value.Title)
	e.u8(status)
	e.integer(value.Order)
	e.timestamp(value.CreatedAt)
	e.timestamp(value.UpdatedAt)
	return e.err
}

func (e *nativeEncoder) note(key []byte, value *model.Note) error {
	if err := validateRecordID(key, value.ID); err != nil {
		return err
	}
	target, err := noteTargetTag(value.Target)
	if err != nil {
		return err
	}
	kind, err := memoryKindTag(value.Kind, true)
	if err != nil {
		return err
	}
	if value.Target == model.TargetProject && value.TargetID != 0 {
		return errors.New("project note target ID must be zero")
	}
	if value.Target != model.TargetProject && value.TargetID == 0 {
		return errors.New("plan or task note target ID must be nonzero")
	}
	e.u64(value.ID)
	e.u8(target)
	e.u64(value.TargetID)
	e.u8(kind)
	e.string(value.Body)
	e.timestamp(value.CreatedAt)
	return e.err
}

func (e *nativeEncoder) milestone(key []byte, value *model.Milestone) error {
	if err := validateRecordID(key, value.ID); err != nil {
		return err
	}
	status, err := milestoneStatusTag(value.Status)
	if err != nil {
		return err
	}
	if value.Order < 0 {
		return errors.New("milestone order must be nonnegative")
	}
	e.u64(value.ID)
	e.string(value.Title)
	e.u8(status)
	e.timestamp(value.Due)
	e.integer(value.Order)
	e.timestamp(value.CreatedAt)
	e.timestamp(value.UpdatedAt)
	return e.err
}

func (e *nativeEncoder) issue(key []byte, value *model.Issue) error {
	if err := validateRecordID(key, value.ID); err != nil {
		return err
	}
	status, err := issueStatusTag(value.Status)
	if err != nil {
		return err
	}
	severity, err := severityTag(value.Severity)
	if err != nil {
		return err
	}
	e.u64(value.ID)
	e.string(value.Title)
	e.string(value.Body)
	e.u8(status)
	e.u8(severity)
	e.u64(value.TaskID)
	e.timestamp(value.CreatedAt)
	e.timestamp(value.UpdatedAt)
	return e.err
}

func (e *nativeEncoder) commit(key []byte, value *model.Commit) error {
	if err := validateRecordID(key, value.ID); err != nil {
		return err
	}
	e.u64(value.ID)
	e.string(value.SHA)
	e.string(value.Subject)
	e.u64(value.PlanID)
	e.u64(value.TaskID)
	e.timestamp(value.CreatedAt)
	return e.err
}

func (e *nativeEncoder) capability(key []byte, value *model.Capability) error {
	if err := validateRecordID(key, value.ID); err != nil {
		return err
	}
	if err := validateCapability(value); err != nil {
		return err
	}
	kind, _ := capabilityKindTag(value.Kind)
	digest, _ := hex.DecodeString(value.ScopeDigest)
	e.u64(value.ID)
	e.u64(uint64(value.ModelVersion))
	e.u64(value.Revision)
	e.string(value.Name)
	e.u8(kind)
	e.string(value.AgentProfile)
	// Legacy grants are deliberately revoked at the trust boundary. The native
	// application must normalize the scope and obtain explicit reapproval.
	e.boolean(false)
	e.i64(value.ApprovalDurationSeconds)
	e.timestamp(time.Time{})
	e.timestamp(time.Time{})
	e.digest32(digest)
	e.capabilityLimits(value.Limits)
	e.capabilityAuditPolicy(value.Audit)
	e.httpScope(value.HTTP)
	e.gitScope(value.Git)
	e.sshScope(value.SSH)
	e.timestamp(value.CreatedAt)
	e.timestamp(value.UpdatedAt)
	return e.err
}

func (e *nativeEncoder) capabilityAudit(key []byte, value *model.CapabilityAudit) error {
	if err := validateRecordID(key, value.ID); err != nil {
		return err
	}
	kind, err := capabilityKindTag(value.Kind)
	if err != nil {
		return err
	}
	if value.CapabilityID == 0 {
		return errors.New("capability audit capability ID must be nonzero")
	}
	if value.DurationMillis < 0 || value.RequestBytes < 0 || value.ResponseBytes < 0 || value.Redirects < 0 {
		return errors.New("capability audit counters must be nonnegative")
	}
	if !validAuditErrorClass(value.Success, value.ErrorClass) {
		return errors.New("capability audit error class is inconsistent")
	}
	e.u64(value.ID)
	e.u64(value.CapabilityID)
	e.string(value.AgentProfile)
	e.u8(kind)
	e.string(value.Operation)
	e.string(value.Target)
	e.boolean(value.Success)
	e.string(value.ErrorClass)
	e.i64(value.DurationMillis)
	e.i64(value.RequestBytes)
	e.i64(value.ResponseBytes)
	e.integer(value.Redirects)
	e.timestamp(value.CreatedAt)
	return e.err
}

func (e *nativeEncoder) memoryWriteback(key []byte, value *memoryWritebackRecord) error {
	if err := validateMemoryWritebackKey(key); err != nil {
		return err
	}
	kind, err := memoryKindTag(value.Kind, false)
	if err != nil {
		return err
	}
	if value.Sequence == 0 {
		return errors.New("memory write-back sequence must be nonzero")
	}
	if value.Digest == ([32]byte{}) {
		return errors.New("memory write-back digest must be nonzero")
	}
	if value.Kind == model.MemorySummary && value.NoteID != 0 {
		return errors.New("summary memory write-back must not contain a note ID")
	}
	if value.Kind != model.MemorySummary && value.NoteID == 0 {
		return errors.New("typed memory write-back must contain a note ID")
	}
	e.digest32(value.Digest[:])
	e.u64(value.Sequence)
	e.u8(kind)
	e.u64(value.NoteID)
	return e.err
}

func (e *nativeEncoder) projectRef(key []byte, value *model.ProjectRef) error {
	if !utf8.Valid(key) || string(key) != value.Path {
		return errors.New("project registry key must equal the project path")
	}
	if err := validateAbsoluteCleanPath(value.Path); err != nil {
		return fmt.Errorf("invalid project path: %w", err)
	}
	if value.Name == "" || !utf8.ValidString(value.Name) {
		return errors.New("project name must be nonempty valid UTF-8")
	}
	e.string(value.Name)
	e.string(value.Path)
	e.timestamp(value.LastSeen)
	return e.err
}

func (e *nativeEncoder) capabilityLimits(value model.CapabilityLimits) {
	e.integer(value.TimeoutSeconds)
	e.i64(value.MaxRequestBytes)
	e.i64(value.MaxResponseBytes)
	e.i64(value.MaxOutputBytes)
	e.integer(value.MaxRedirects)
	e.integer(value.MaxConcurrent)
}

func (e *nativeEncoder) capabilityAuditPolicy(value model.CapabilityAuditPolicy) {
	e.boolean(value.Enabled)
	e.integer(value.RetainLast)
}

func (e *nativeEncoder) httpScope(value *model.HTTPScope) {
	e.option(value != nil)
	if value == nil {
		return
	}
	e.string(value.BaseURL)
	e.strings(value.Methods)
	e.strings(value.PathPrefixes)
}

func (e *nativeEncoder) gitScope(value *model.GitScope) {
	e.option(value != nil)
	if value == nil {
		return
	}
	e.string(value.RemoteName)
	e.string(value.RemoteURL)
	e.strings(value.Operations)
	e.strings(value.Branches)
	e.strings(value.Refspecs)
	e.boolean(value.AllowTags)
	e.boolean(value.AllowForcePush)
	e.boolean(value.AllowDeleteRefs)
}

func (e *nativeEncoder) sshScope(value *model.SSHScope) {
	e.option(value != nil)
	if value == nil {
		return
	}
	e.string(value.Alias)
	e.string(value.Host)
	e.u16(value.Port)
	e.string(value.User)
	e.string(value.HostKey)
	e.boolean(value.AllowGit)
	e.strings(value.RemoteCommands)
	e.boolean(value.AllowUpload)
	e.boolean(value.AllowDownload)
	e.strings(value.UploadRoots)
	e.strings(value.DownloadRoots)
	e.strings(value.UploadRemoteRoots)
	e.strings(value.DownloadRemoteRoots)
	e.boolean(value.AllowInteractiveShell)
	e.strings(value.LocalForwardTargets)
	e.strings(value.RemoteForwardTargets)
}

func (e *nativeEncoder) u8(value uint8) {
	e.write([]byte{value})
}

func (e *nativeEncoder) boolean(value bool) {
	if value {
		e.u8(1)
	} else {
		e.u8(0)
	}
}

func (e *nativeEncoder) option(present bool) { e.boolean(present) }

func (e *nativeEncoder) u16(value uint16) {
	encoded := [2]byte{}
	binary.BigEndian.PutUint16(encoded[:], value)
	e.write(encoded[:])
}

func (e *nativeEncoder) u32(value uint32) {
	encoded := [4]byte{}
	binary.BigEndian.PutUint32(encoded[:], value)
	e.write(encoded[:])
}

func (e *nativeEncoder) i32(value int32) {
	e.u32(uint32(value))
}

func (e *nativeEncoder) u64(value uint64) {
	encoded := [8]byte{}
	binary.BigEndian.PutUint64(encoded[:], value)
	e.write(encoded[:])
}

func (e *nativeEncoder) i64(value int64) {
	e.u64(uint64(value))
}

func (e *nativeEncoder) integer(value int) { e.i64(int64(value)) }

func (e *nativeEncoder) string(value string) {
	if e.err != nil {
		return
	}
	if !utf8.ValidString(value) {
		e.err = errors.New("string is not valid UTF-8")
		return
	}
	if len(value) > nativeMaxPayload || uint64(len(value)) > math.MaxUint32 {
		e.err = errors.New("string exceeds u32 length")
		return
	}
	e.u32(uint32(len(value)))
	if e.err == nil {
		e.write([]byte(value))
	}
}

func (e *nativeEncoder) strings(values []string) {
	if len(values) > nativeMaxListItems || uint64(len(values)) > math.MaxUint32 {
		e.err = errors.New("string list exceeds u32 length")
		return
	}
	e.u32(uint32(len(values)))
	for _, value := range values {
		e.string(value)
	}
}

func (e *nativeEncoder) timestamp(value time.Time) {
	if value.IsZero() {
		e.u8(0)
		return
	}
	_, offset := value.Zone()
	if value.Nanosecond() < 0 || value.Nanosecond() >= 1_000_000_000 {
		e.err = errors.New("timestamp nanoseconds are outside one second")
		return
	}
	if offset < -86_400 || offset > 86_400 {
		e.err = errors.New("timestamp UTC offset exceeds 24 hours")
		return
	}
	e.u8(1)
	e.i64(value.Unix())
	e.u32(uint32(value.Nanosecond()))
	e.i32(int32(offset))
}

func (e *nativeEncoder) digest32(value []byte) {
	if e.err != nil {
		return
	}
	if len(value) != 32 {
		e.err = fmt.Errorf("digest has %d bytes, want 32", len(value))
		return
	}
	e.write(value)
}

func (e *nativeEncoder) write(value []byte) {
	if e.err != nil {
		return
	}
	if len(value) > nativeMaxPayload-e.buffer.Len() {
		e.err = errors.New("native payload exceeds maximum length")
		return
	}
	_, e.err = e.buffer.Write(value)
}

func validateRecordID(key []byte, id uint64) error {
	if len(key) != 8 {
		return fmt.Errorf("numeric key has %d bytes, want 8", len(key))
	}
	if id == 0 {
		return errors.New("record ID must be nonzero")
	}
	if keyID := binary.BigEndian.Uint64(key); keyID != id {
		return fmt.Errorf("numeric key %d does not match record ID %d", keyID, id)
	}
	return nil
}

func validateMemoryWritebackKey(key []byte) error {
	if len(key) == 0 || len(key) > maxMemoryWritebackRequestID || !utf8.Valid(key) {
		return errors.New("invalid memory write-back request ID")
	}
	for _, value := range key {
		if !(value >= 'a' && value <= 'z' || value >= 'A' && value <= 'Z' ||
			value >= '0' && value <= '9' || value == '-' || value == '_' ||
			value == '.' || value == ':') {
			return errors.New("invalid memory write-back request ID")
		}
	}
	return nil
}

func validateCapability(value *model.Capability) error {
	if value.ModelVersion != model.CapabilityModelVersion {
		return fmt.Errorf("unsupported capability model version %d", value.ModelVersion)
	}
	if value.Revision == 0 {
		return errors.New("capability revision must be nonzero")
	}
	if err := validateCanonicalText("capability name", value.Name, 128); err != nil {
		return err
	}
	if err := validateCanonicalText("agent profile", value.AgentProfile, 64); err != nil {
		return err
	}
	if _, err := capabilityKindTag(value.Kind); err != nil {
		return err
	}
	scopes := 0
	if value.HTTP != nil {
		scopes++
	}
	if value.Git != nil {
		scopes++
	}
	if value.SSH != nil {
		scopes++
	}
	if scopes != 1 || value.Kind == model.CapabilityHTTP && value.HTTP == nil ||
		value.Kind == model.CapabilityGit && value.Git == nil ||
		value.Kind == model.CapabilitySSH && value.SSH == nil {
		return errors.New("capability must contain exactly its kind-specific scope")
	}
	if value.ApprovalDurationSeconds < 60 || value.ApprovalDurationSeconds > 30*24*60*60 {
		return errors.New("capability approval duration is outside the supported range")
	}
	if value.Limits.TimeoutSeconds < 1 || value.Limits.TimeoutSeconds > 300 ||
		value.Limits.MaxRequestBytes < 1 || value.Limits.MaxRequestBytes > 32<<20 ||
		value.Limits.MaxResponseBytes < 1 || value.Limits.MaxResponseBytes > 32<<20 ||
		value.Limits.MaxOutputBytes < 1 || value.Limits.MaxOutputBytes > 32<<20 ||
		value.Limits.MaxRedirects < 0 || value.Limits.MaxRedirects > 10 ||
		value.Limits.MaxConcurrent < 1 || value.Limits.MaxConcurrent > 8 {
		return errors.New("capability limits are not normalized")
	}
	if value.Audit.RetainLast < 0 || value.Audit.RetainLast > 1000 {
		return errors.New("capability audit retention is outside the supported range")
	}
	if len(value.ScopeDigest) != 64 || strings.ToLower(value.ScopeDigest) != value.ScopeDigest {
		return errors.New("capability scope digest must be 64 lowercase hexadecimal characters")
	}
	digest, err := hex.DecodeString(value.ScopeDigest)
	if err != nil {
		return errors.New("capability scope digest must be 64 lowercase hexadecimal characters")
	}
	if bytes.Equal(digest, make([]byte, 32)) {
		return errors.New("capability scope digest must be nonzero")
	}
	if value.Enabled {
		if value.ApprovedAt.IsZero() || value.ExpiresAt.IsZero() || !value.ExpiresAt.After(value.ApprovedAt) {
			return errors.New("enabled capability has an invalid approval window")
		}
		maximum := value.ApprovedAt.Add(time.Duration(value.ApprovalDurationSeconds) * time.Second)
		if value.ExpiresAt.After(maximum) {
			return errors.New("enabled capability approval exceeds its duration")
		}
	} else if !value.ApprovedAt.IsZero() || !value.ExpiresAt.IsZero() {
		return errors.New("disabled capability retains approval timestamps")
	}
	return validateCapabilityStrings(value)
}

func validateCapabilityStrings(value *model.Capability) error {
	stringsToCheck := []string{value.Name, value.AgentProfile}
	if value.HTTP != nil {
		stringsToCheck = append(stringsToCheck, value.HTTP.BaseURL)
		stringsToCheck = append(stringsToCheck, value.HTTP.Methods...)
		stringsToCheck = append(stringsToCheck, value.HTTP.PathPrefixes...)
	}
	if value.Git != nil {
		stringsToCheck = append(stringsToCheck, value.Git.RemoteName, value.Git.RemoteURL)
		stringsToCheck = append(stringsToCheck, value.Git.Operations...)
		stringsToCheck = append(stringsToCheck, value.Git.Branches...)
		stringsToCheck = append(stringsToCheck, value.Git.Refspecs...)
	}
	if value.SSH != nil {
		stringsToCheck = append(stringsToCheck, value.SSH.Alias, value.SSH.Host, value.SSH.User, value.SSH.HostKey)
		stringsToCheck = append(stringsToCheck, value.SSH.RemoteCommands...)
		stringsToCheck = append(stringsToCheck, value.SSH.UploadRoots...)
		stringsToCheck = append(stringsToCheck, value.SSH.DownloadRoots...)
		stringsToCheck = append(stringsToCheck, value.SSH.UploadRemoteRoots...)
		stringsToCheck = append(stringsToCheck, value.SSH.DownloadRemoteRoots...)
		stringsToCheck = append(stringsToCheck, value.SSH.LocalForwardTargets...)
		stringsToCheck = append(stringsToCheck, value.SSH.RemoteForwardTargets...)
	}
	for _, item := range stringsToCheck {
		if !utf8.ValidString(item) {
			return errors.New("capability contains invalid UTF-8")
		}
	}
	if value.HTTP != nil && (value.HTTP.BaseURL == "" || len(value.HTTP.Methods) == 0 || len(value.HTTP.PathPrefixes) == 0) {
		return errors.New("HTTP capability scope is incomplete")
	}
	if value.Git != nil && (value.Git.RemoteName == "" || value.Git.RemoteURL == "" || len(value.Git.Operations) == 0) {
		return errors.New("Git capability scope is incomplete")
	}
	if value.SSH != nil && (value.SSH.Host == "" || value.SSH.Port == 0 || value.SSH.User == "" ||
		value.SSH.HostKey == "" || value.SSH.AllowInteractiveShell) {
		return errors.New("SSH capability scope is incomplete or unsupported")
	}
	return nil
}

func validateCanonicalText(name, value string, maximum int) error {
	if value == "" || len(value) > maximum || strings.TrimSpace(value) != value || !utf8.ValidString(value) {
		return fmt.Errorf("%s is not canonical UTF-8 text", name)
	}
	for _, character := range value {
		if character < 0x20 || character == 0x7f {
			return fmt.Errorf("%s contains control characters", name)
		}
	}
	return nil
}

func validAuditErrorClass(success bool, value string) bool {
	if success {
		return value == "none"
	}
	switch value {
	case "denied", "dns", "routing", "vpn", "proxy", "tls", "host-key",
		"authentication", "sandbox", "remote-policy", "timeout", "transport",
		"request-limit", "response-limit", "output-limit", "cancelled", "internal":
		return true
	default:
		return false
	}
}

func validateGlobalConfig(key, value []byte) error {
	if len(key) == 0 || !utf8.Valid(key) || !utf8.Valid(value) {
		return errors.New("global config key and value must be nonempty-key valid UTF-8")
	}
	return nil
}

func validateGlobalBackup(key, value []byte) error {
	if !utf8.Valid(key) || !utf8.Valid(value) {
		return errors.New("global backup key and value must be valid UTF-8")
	}
	if string(key) == "" || strings.HasPrefix(string(key), "+") {
		return errors.New("global backup key must be a canonical nonnegative decimal timestamp")
	}
	timestamp, err := strconv.ParseInt(string(key), 10, 64)
	if err != nil || timestamp < 0 || strconv.FormatInt(timestamp, 10) != string(key) {
		return errors.New("global backup key must be a canonical nonnegative decimal timestamp")
	}
	parts := strings.Split(string(value), "\t")
	if len(parts) != 2 {
		return errors.New("global backup value must contain exactly project and backup paths")
	}
	if err := validateAbsoluteCleanPath(parts[0]); err != nil {
		return fmt.Errorf("invalid backup project path: %w", err)
	}
	if err := validateAbsoluteCleanPath(parts[1]); err != nil {
		return fmt.Errorf("invalid backup destination path: %w", err)
	}
	return nil
}

func validateAbsoluteCleanPath(value string) error {
	if value == "" || !utf8.ValidString(value) || !filepath.IsAbs(value) {
		return errors.New("path must be absolute valid UTF-8")
	}
	if filepath.Clean(value) != value {
		return errors.New("path must be lexically clean")
	}
	return nil
}

func planStatusTag(value model.PlanStatus) (uint8, error) {
	switch value {
	case model.PlanActive:
		return 1, nil
	case model.PlanDone:
		return 2, nil
	case model.PlanArchived:
		return 3, nil
	default:
		return 0, fmt.Errorf("unknown plan status %q", value)
	}
}

func taskStatusTag(value model.TaskStatus) (uint8, error) {
	switch value {
	case model.TaskTodo:
		return 1, nil
	case model.TaskDoing:
		return 2, nil
	case model.TaskDone:
		return 3, nil
	case model.TaskBlocked:
		return 4, nil
	default:
		return 0, fmt.Errorf("unknown task status %q", value)
	}
}

func noteTargetTag(value model.NoteTarget) (uint8, error) {
	switch value {
	case model.TargetProject:
		return 1, nil
	case model.TargetPlan:
		return 2, nil
	case model.TargetTask:
		return 3, nil
	default:
		return 0, fmt.Errorf("unknown note target %q", value)
	}
}

func memoryKindTag(value model.MemoryKind, allowLegacyEmpty bool) (uint8, error) {
	switch value {
	case "":
		if allowLegacyEmpty {
			return 0, nil
		}
	case model.MemoryDecision:
		return 1, nil
	case model.MemoryBlocker:
		return 2, nil
	case model.MemoryHandoff:
		return 3, nil
	case model.MemorySummary:
		if !allowLegacyEmpty {
			return 4, nil
		}
	}
	return 0, fmt.Errorf("unsupported memory kind %q", value)
}

func milestoneStatusTag(value model.MilestoneStatus) (uint8, error) {
	switch value {
	case model.MilestoneOpen:
		return 1, nil
	case model.MilestoneDone:
		return 2, nil
	default:
		return 0, fmt.Errorf("unknown milestone status %q", value)
	}
}

func issueStatusTag(value model.IssueStatus) (uint8, error) {
	switch value {
	case model.IssueOpen:
		return 1, nil
	case model.IssueClosed:
		return 2, nil
	default:
		return 0, fmt.Errorf("unknown issue status %q", value)
	}
}

func severityTag(value model.Severity) (uint8, error) {
	switch value {
	case model.SeverityLow:
		return 1, nil
	case model.SeverityMedium:
		return 2, nil
	case model.SeverityHigh:
		return 3, nil
	case model.SeverityCritical:
		return 4, nil
	default:
		return 0, fmt.Errorf("unknown issue severity %q", value)
	}
}

func capabilityKindTag(value model.CapabilityKind) (uint8, error) {
	switch value {
	case model.CapabilityHTTP:
		return 1, nil
	case model.CapabilityGit:
		return 2, nil
	case model.CapabilitySSH:
		return 3, nil
	default:
		return 0, fmt.Errorf("unknown capability kind %q", value)
	}
}
