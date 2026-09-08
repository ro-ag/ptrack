# Deterministic Stack Discovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the misleading Overview "Lines of code" tile with a deterministic, manifest-driven discovery of the project's languages and subprojects, counted in tracked files.

**Architecture:** A pure resolver in `ptrack-core` turns a sorted list of tracked repository paths into a `StackProfile` using a fixed marker table. `ptrack-git` collects that path list with one bounded `git ls-files -z` call. `ptrack-app` decides when to scan (project open, and any HEAD change), persists the full profile on `Meta.stack` and a compact summary on the global `ProjectRef`, and serves both to the desktop. The CLI context digest and the recent-project cards read the persisted values.

**Tech Stack:** Rust (workspace crates `ptrack-core`, `ptrack-store`, `ptrack-git`, `ptrack-app`, `ptrack-cli`), redb via the existing store layer, vanilla ES modules + TypeScript for the frontend, Vitest for frontend tests.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-09-07-stack-discovery-design.md`. Every requirement there applies to every task.
- Never put `#[cfg(test)] mod tests` inside a source file. Tests go in a sibling `module_test.rs`, declared from `lib.rs` with `#[cfg(test)] mod module_test;`.
- Never add AI attribution to commits.
- Work happens on the branch `feat/stack-discovery`. Never commit to `main`.
- No new dependencies. Everything uses the existing workspace crates and the git binary already invoked through `ptrack-git`.
- `STORE_SCHEMA_VERSION` must stay at `4`. Persistence is additive at the payload-schema level only.
- `NATIVE_PAYLOAD_SCHEMA` moves from `4` to `5`. `MIN_NATIVE_PAYLOAD_SCHEMA` stays at `1`.
- Bytes and lines are never counted, stored, or displayed. Counts are tracked-file integers.
- Bounds: at most 64 discovered projects per profile, at most 8 evidence paths per project, at most 200,000 tracked paths per scan.
- Local gate before any merge: `make test` (runs `cargo fmt --all -- --check`, `cargo test --workspace --all-targets --no-fail-fast`, `cargo clippy --workspace --all-targets -- -D warnings`, the doc build, and `tools/help_check.py all`).

## File Structure

- Create `crates/ptrack-core/src/stack.rs` — marker table and the pure resolver. One responsibility: paths in, profile out.
- Create `crates/ptrack-core/src/stack_test.rs` — resolver fixtures, golden profiles, determinism test.
- Modify `crates/ptrack-core/src/model.rs` — `LanguageId`, `StackProject`, `StackProfile`, `StackSummary`, and the new `Meta.stack` / `ProjectRef.stack` fields.
- Modify `crates/ptrack-core/src/codec.rs` — payload schema 5 encode/decode for both new fields.
- Modify `crates/ptrack-core/src/lib.rs` — module wiring and re-exports.
- Modify `crates/ptrack-store/src/project.rs` — read/write the profile through `Meta`.
- Modify `crates/ptrack-store/src/global.rs` — write the summary; preserve it across `register_project`.
- Create `crates/ptrack-git/src/tracked.rs` + `tracked_test.rs` — the `git ls-files` collector and its cap.
- Modify `crates/ptrack-git/src/runner.rs` — a production constructor with explicit limits.
- Modify `crates/ptrack-app/src/desktop_runtime.rs` — scan orchestration, the new command, removal of `repo_stats`.
- Modify `crates/ptrack-cli/src/dispatch.rs` — the context digest stack section.
- Modify `frontend/src/app.js`, `frontend/src/workspace/recent-projects.ts` — tiles, panel, card labels.

---

### Task 1: Core stack types and pure resolver

**Files:**
- Create: `crates/ptrack-core/src/stack.rs`
- Create: `crates/ptrack-core/src/stack_test.rs`
- Modify: `crates/ptrack-core/src/model.rs` (append after the `Counts` struct, around line 602)
- Modify: `crates/ptrack-core/src/lib.rs` (module list and the `pub use` block)

**Interfaces:**
- Produces: `LanguageId` (persistent enum, `wire_tag`/`as_str`/`from_name`), `StackProject { root: String, language: LanguageId, evidence: Vec<String>, depth: u8, files: u32 }`, `StackProfile { projects: Vec<StackProject>, scanned_head: String, scanned_at: Timestamp, tracked_files: u32, incomplete: bool }`, `StackSummary { languages: Vec<(LanguageId, u32)>, tracked_files: u32, scanned_head: String, incomplete: bool }`, `ptrack_core::stack::resolve(paths: &[String]) -> Vec<StackProject>`, `ptrack_core::stack::summarize(profile: &StackProfile) -> StackSummary`, and the constants `MAX_STACK_PROJECTS: usize = 64`, `MAX_STACK_EVIDENCE: usize = 8`.

- [ ] **Step 1: Write the failing test**

Create `crates/ptrack-core/src/stack_test.rs`:

```rust
use crate::model::LanguageId;
use crate::stack::{MAX_STACK_PROJECTS, resolve};

fn paths(values: &[&str]) -> Vec<String> {
    let mut owned: Vec<String> = values.iter().map(|value| (*value).to_owned()).collect();
    owned.sort();
    owned
}

#[test]
fn a_root_manifest_defines_one_project_with_its_evidence() {
    let resolved = resolve(&paths(&["Cargo.toml", "src/lib.rs", "src/main.rs"]));
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].root, "");
    assert_eq!(resolved[0].language, LanguageId::Rust);
    assert_eq!(resolved[0].evidence, vec!["Cargo.toml".to_owned()]);
    assert_eq!(resolved[0].depth, 0);
    assert_eq!(resolved[0].files, 3);
}

#[test]
fn every_tracked_manifest_defines_its_own_project_ranked_shallowest_first() {
    let resolved = resolve(&paths(&[
        "Cargo.toml",
        "crates/one/Cargo.toml",
        "crates/one/src/lib.rs",
        "frontend/package.json",
        "frontend/src/app.js",
        "src/lib.rs",
    ]));
    let roots: Vec<&str> = resolved.iter().map(|project| project.root.as_str()).collect();
    assert_eq!(roots, vec!["", "crates/one", "frontend"]);
    assert_eq!(resolved[1].files, 2);
    assert_eq!(resolved[2].language, LanguageId::JavaScript);
}

#[test]
fn a_tracked_tsconfig_refines_javascript_to_typescript() {
    let resolved = resolve(&paths(&[
        "frontend/package.json",
        "frontend/tsconfig.json",
        "frontend/src/app.ts",
    ]));
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].language, LanguageId::TypeScript);
    assert_eq!(
        resolved[0].evidence,
        vec!["frontend/package.json".to_owned(), "frontend/tsconfig.json".to_owned()]
    );
}

#[test]
fn files_are_attributed_to_the_nearest_enclosing_project() {
    let resolved = resolve(&paths(&[
        "Cargo.toml",
        "crates/one/Cargo.toml",
        "crates/one/src/lib.rs",
        "crates/one/src/model.rs",
        "src/lib.rs",
    ]));
    assert_eq!(resolved[0].files, 2);
    assert_eq!(resolved[1].files, 3);
}

#[test]
fn resolution_is_independent_of_input_order() {
    let ordered = paths(&[
        "Cargo.toml",
        "crates/one/Cargo.toml",
        "crates/one/src/lib.rs",
        "frontend/package.json",
        "go.mod",
    ]);
    let expected = resolve(&ordered);
    let mut rotated = ordered.clone();
    rotated.rotate_left(2);
    let mut reversed = ordered;
    reversed.reverse();
    assert_eq!(resolve(&rotated), expected);
    assert_eq!(resolve(&reversed), expected);
}

#[test]
fn discovered_projects_are_capped_and_the_shallowest_survive() {
    let mut values: Vec<String> = (0..MAX_STACK_PROJECTS + 10)
        .map(|index| format!("crates/c{index:03}/Cargo.toml"))
        .collect();
    values.push("Cargo.toml".to_owned());
    values.sort();
    let resolved = resolve(&values);
    assert_eq!(resolved.len(), MAX_STACK_PROJECTS);
    assert_eq!(resolved[0].root, "");
}

#[test]
fn a_tree_with_no_manifest_resolves_to_no_projects() {
    assert!(resolve(&paths(&["README.md", "docs/guide.md"])).is_empty());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --package ptrack-core stack_test`
Expected: FAIL — `unresolved import crate::stack`.

- [ ] **Step 3: Add the model types**

In `crates/ptrack-core/src/model.rs`, after the `Counts` struct (ends line 602), add:

```rust
persistent_enum!(LanguageId {
    Rust = 1 => "rust",
    Go = 2 => "go",
    JavaScript = 3 => "javascript",
    TypeScript = 4 => "typescript",
    Python = 5 => "python",
    Swift = 6 => "swift",
    Java = 7 => "java",
    Kotlin = 8 => "kotlin",
    CSharp = 9 => "csharp",
    Ruby = 10 => "ruby",
    Php = 11 => "php",
    Elixir = 12 => "elixir",
    Dart = 13 => "dart",
    C = 14 => "c",
    Terraform = 15 => "terraform",
    Container = 16 => "container",
});

/// One project discovered by a tracked manifest.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StackProject {
    /// Repository-relative directory holding the manifest; empty at the root.
    pub root: String,
    pub language: LanguageId,
    /// Manifest paths that produced this project, sorted, at most
    /// [`MAX_STACK_EVIDENCE`].
    pub evidence: Vec<String>,
    pub depth: u8,
    /// Tracked files attributed to this project.
    pub files: u32,
}

/// The durable result of one tracked-file scan.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StackProfile {
    /// Discovered projects, shallowest first; at most [`MAX_STACK_PROJECTS`].
    pub projects: Vec<StackProject>,
    /// HEAD the scan ran against; a rescan is due when it no longer matches.
    pub scanned_head: String,
    pub scanned_at: Timestamp,
    pub tracked_files: u32,
    /// The tracked path listing hit the scan cap and was truncated.
    pub incomplete: bool,
}

/// The compact per-project summary carried by the global registry.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StackSummary {
    /// Languages with their file counts, ranked by the profile's own order.
    pub languages: Vec<(LanguageId, u32)>,
    pub tracked_files: u32,
    pub scanned_head: String,
    pub incomplete: bool,
}
```

`LanguageId` needs a `Default` for `StackProject::default()`; add it right below the enum:

```rust
impl Default for LanguageId {
    fn default() -> Self {
        Self::Rust
    }
}
```

- [ ] **Step 4: Write the resolver**

Create `crates/ptrack-core/src/stack.rs`:

```rust
//! Deterministic project discovery.
//!
//! The resolver is a pure function of a sorted tracked-path list. It performs
//! no filesystem access, runs no subprocess, and reads no clock, so the same
//! listing always produces the same profile. Language identity comes from the
//! marker table below — never from how common an extension is in the tree.

use std::collections::BTreeMap;

use crate::model::{LanguageId, StackProfile, StackProject, StackSummary};

/// The most discovered projects one profile carries.
pub const MAX_STACK_PROJECTS: usize = 64;
/// The most evidence paths one discovered project carries.
pub const MAX_STACK_EVIDENCE: usize = 8;

/// How a marker matches a tracked path's final component.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Marker {
    /// The exact file name.
    Name(&'static str),
    /// The file name's extension, without its dot.
    Extension(&'static str),
}

/// The fixed, ordered marker table. Order is part of the ranking contract:
/// when two markers sit in the same directory, the earlier entry wins.
const MARKERS: &[(Marker, LanguageId)] = &[
    (Marker::Name("Cargo.toml"), LanguageId::Rust),
    (Marker::Name("go.mod"), LanguageId::Go),
    (Marker::Name("package.json"), LanguageId::JavaScript),
    (Marker::Name("pyproject.toml"), LanguageId::Python),
    (Marker::Name("setup.py"), LanguageId::Python),
    (Marker::Name("Package.swift"), LanguageId::Swift),
    (Marker::Name("pom.xml"), LanguageId::Java),
    (Marker::Name("build.gradle"), LanguageId::Java),
    (Marker::Name("build.gradle.kts"), LanguageId::Kotlin),
    (Marker::Extension("csproj"), LanguageId::CSharp),
    (Marker::Name("Gemfile"), LanguageId::Ruby),
    (Marker::Name("composer.json"), LanguageId::Php),
    (Marker::Name("mix.exs"), LanguageId::Elixir),
    (Marker::Name("pubspec.yaml"), LanguageId::Dart),
    (Marker::Name("CMakeLists.txt"), LanguageId::C),
    (Marker::Extension("tf"), LanguageId::Terraform),
    (Marker::Name("Dockerfile"), LanguageId::Container),
];

/// The refinement marker: a tracked `tsconfig.json` beside a `package.json`
/// makes the project TypeScript rather than JavaScript.
const TYPESCRIPT_MARKER: &str = "tsconfig.json";

/// Resolves discovered projects from a sorted tracked-path list.
///
/// Paths are repository-relative and use `/` separators. The result does not
/// depend on the order they arrive in.
#[must_use]
pub fn resolve(paths: &[String]) -> Vec<StackProject> {
    let mut discovered: BTreeMap<String, (usize, LanguageId, Vec<String>)> = BTreeMap::new();
    let mut typescript_roots: Vec<String> = Vec::new();

    for path in paths {
        let (directory, name) = split_path(path);
        if name == TYPESCRIPT_MARKER {
            typescript_roots.push(directory.to_owned());
        }
        let Some((index, language)) = match_marker(name) else {
            continue;
        };
        let entry = discovered
            .entry(directory.to_owned())
            .or_insert_with(|| (index, language, Vec::new()));
        if index < entry.0 {
            entry.0 = index;
            entry.1 = language;
        }
        entry.2.push(path.clone());
    }

    let mut projects: Vec<StackProject> = discovered
        .into_iter()
        .map(|(root, (_, language, mut evidence))| {
            let language = if language == LanguageId::JavaScript
                && typescript_roots.iter().any(|value| *value == root)
            {
                LanguageId::TypeScript
            } else {
                language
            };
            if language == LanguageId::TypeScript {
                let marker = join_path(&root, TYPESCRIPT_MARKER);
                if !evidence.contains(&marker) {
                    evidence.push(marker);
                }
            }
            // Sort before truncating: which evidence survives the cap must not
            // depend on the order the collector happened to report paths in.
            evidence.sort();
            evidence.truncate(MAX_STACK_EVIDENCE);
            let depth = u8::try_from(depth_of(&root)).unwrap_or(u8::MAX);
            StackProject {
                root,
                language,
                evidence,
                depth,
                files: 0,
            }
        })
        .collect();

    projects.sort_by(|left, right| {
        left.depth
            .cmp(&right.depth)
            .then_with(|| marker_rank(left.language).cmp(&marker_rank(right.language)))
            .then_with(|| left.root.cmp(&right.root))
    });
    projects.truncate(MAX_STACK_PROJECTS);

    attribute_files(&mut projects, paths);
    projects
}

/// Summarizes a profile for the global registry: language totals in profile
/// order, collapsing repeated languages into one entry.
#[must_use]
pub fn summarize(profile: &StackProfile) -> StackSummary {
    let mut languages: Vec<(LanguageId, u32)> = Vec::new();
    for project in &profile.projects {
        if let Some(entry) = languages
            .iter_mut()
            .find(|(language, _)| *language == project.language)
        {
            entry.1 = entry.1.saturating_add(project.files);
        } else {
            languages.push((project.language, project.files));
        }
    }
    StackSummary {
        languages,
        tracked_files: profile.tracked_files,
        scanned_head: profile.scanned_head.clone(),
        incomplete: profile.incomplete,
    }
}

/// Attributes every tracked path to the deepest project whose root is a prefix
/// of it.
fn attribute_files(projects: &mut [StackProject], paths: &[String]) {
    for path in paths {
        let mut best: Option<usize> = None;
        for (index, project) in projects.iter().enumerate() {
            if !contains(&project.root, path) {
                continue;
            }
            let deeper = best.is_none_or(|current| projects[current].root.len() < project.root.len());
            if deeper {
                best = Some(index);
            }
        }
        if let Some(index) = best {
            projects[index].files = projects[index].files.saturating_add(1);
        }
    }
}

fn contains(root: &str, path: &str) -> bool {
    if root.is_empty() {
        return true;
    }
    path.len() > root.len()
        && path.starts_with(root)
        && path.as_bytes().get(root.len()) == Some(&b'/')
}

fn match_marker(name: &str) -> Option<(usize, LanguageId)> {
    MARKERS
        .iter()
        .enumerate()
        .find(|(_, (marker, _))| match marker {
            Marker::Name(value) => *value == name,
            Marker::Extension(value) => name
                .rsplit_once('.')
                .is_some_and(|(_, extension)| extension == *value),
        })
        .map(|(index, (_, language))| (index, *language))
}

fn marker_rank(language: LanguageId) -> usize {
    MARKERS
        .iter()
        .position(|(_, value)| *value == language)
        .unwrap_or(MARKERS.len())
}

fn split_path(path: &str) -> (&str, &str) {
    path.rsplit_once('/').map_or(("", path), |(directory, name)| (directory, name))
}

fn join_path(root: &str, name: &str) -> String {
    if root.is_empty() {
        name.to_owned()
    } else {
        format!("{root}/{name}")
    }
}

fn depth_of(root: &str) -> usize {
    if root.is_empty() {
        0
    } else {
        root.matches('/').count() + 1
    }
}
```

`TypeScript` is not in `MARKERS`, so `marker_rank` returns `MARKERS.len()` for it. Give it the JavaScript rank instead:

```rust
fn marker_rank(language: LanguageId) -> usize {
    let language = if language == LanguageId::TypeScript {
        LanguageId::JavaScript
    } else {
        language
    };
    MARKERS
        .iter()
        .position(|(_, value)| *value == language)
        .unwrap_or(MARKERS.len())
}
```

- [ ] **Step 5: Wire the module**

In `crates/ptrack-core/src/lib.rs`, add `pub mod stack;` beside the other module declarations, `#[cfg(test)] mod stack_test;` beside the other test declarations, and extend the `pub use model::{…}` list with `LanguageId, StackProfile, StackProject, StackSummary`.

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test --package ptrack-core stack_test`
Expected: PASS, 7 tests.

- [ ] **Step 7: Lint and commit**

```bash
cargo fmt --all
cargo clippy --package ptrack-core --all-targets -- -D warnings
git add crates/ptrack-core/src/stack.rs crates/ptrack-core/src/stack_test.rs crates/ptrack-core/src/model.rs crates/ptrack-core/src/lib.rs
git commit -m "feat: deterministic stack resolver over tracked paths"
```

---

### Task 2: Payload schema 5 persistence for the profile and summary

**Files:**
- Modify: `crates/ptrack-core/src/codec.rs:14` (`NATIVE_PAYLOAD_SCHEMA`), `:504-529` (meta), `:1173-1185` (project ref)
- Modify: `crates/ptrack-core/src/model.rs` (`Meta`, `ProjectRef`)
- Modify: `crates/ptrack-core/src/codec_test.rs`

**Interfaces:**
- Consumes: `StackProfile`, `StackSummary`, `LanguageId` from Task 1.
- Produces: `Meta.stack: Option<StackProfile>`, `ProjectRef.stack: Option<StackSummary>`, `NATIVE_PAYLOAD_SCHEMA = 5`, and the constant `STACK_PAYLOAD_SCHEMA: u32 = 5` in `codec.rs`.

- [ ] **Step 1: Write the failing test**

Append to `crates/ptrack-core/src/codec_test.rs`:

```rust
#[test]
fn a_meta_stack_profile_round_trips_at_the_native_schema() {
    let mut meta = sample_meta();
    meta.stack = Some(StackProfile {
        projects: vec![StackProject {
            root: "crates/one".to_owned(),
            language: LanguageId::Rust,
            evidence: vec!["crates/one/Cargo.toml".to_owned()],
            depth: 2,
            files: 12,
        }],
        scanned_head: "abc123".to_owned(),
        scanned_at: Timestamp::Zero,
        tracked_files: 12,
        incomplete: false,
    });
    let encoded = encode_record(&NativeRecord::Meta(meta.clone())).expect("encode");
    assert_eq!(decode_record(RecordKind::Meta, &encoded).expect("decode"), NativeRecord::Meta(meta));
}

#[test]
fn a_meta_written_before_schema_five_decodes_without_a_stack() {
    let mut meta = sample_meta();
    meta.stack = None;
    let encoded = encode_record_at_schema(&NativeRecord::Meta(meta.clone()), 4).expect("encode");
    assert_eq!(
        decode_record_at_schema(RecordKind::Meta, 4, &encoded).expect("decode"),
        NativeRecord::Meta(meta)
    );
}

#[test]
fn encoding_a_stack_below_schema_five_is_non_canonical() {
    let mut meta = sample_meta();
    meta.stack = Some(StackProfile::default());
    assert_eq!(
        encode_record_at_schema(&NativeRecord::Meta(meta), 4),
        Err(CodecError::NonCanonical)
    );
}

#[test]
fn a_project_ref_stack_summary_round_trips_and_is_absent_before_schema_five() {
    let summary = StackSummary {
        languages: vec![(LanguageId::Rust, 214), (LanguageId::TypeScript, 38)],
        tracked_files: 252,
        scanned_head: "abc123".to_owned(),
        incomplete: false,
    };
    let value = ProjectRef {
        name: "ptrack".to_owned(),
        path: "/tmp/ptrack".to_owned(),
        last_seen: Timestamp::Zero,
        stack: Some(summary),
    };
    let encoded = encode_record(&NativeRecord::ProjectRef(value.clone())).expect("encode");
    assert_eq!(
        decode_record(RecordKind::ProjectRef, &encoded).expect("decode"),
        NativeRecord::ProjectRef(value.clone())
    );

    let legacy = ProjectRef { stack: None, ..value };
    let encoded = encode_record_at_schema(&NativeRecord::ProjectRef(legacy.clone()), 4).expect("encode");
    assert_eq!(
        decode_record_at_schema(RecordKind::ProjectRef, 4, &encoded).expect("decode"),
        NativeRecord::ProjectRef(legacy)
    );
}
```

Reuse whatever `sample_meta`-style helper already exists in `codec_test.rs`; if there is none, build the `Meta` inline exactly as the neighbouring tests do, and add the needed names to that file's `use` list (`LanguageId`, `StackProfile`, `StackProject`, `StackSummary`).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --package ptrack-core codec_test`
Expected: FAIL — `Meta` has no field named `stack`.

- [ ] **Step 3: Add the fields and the codec**

In `model.rs`, add to `Meta` (after `actors`):

```rust
    /// The most recent deterministic stack scan. `None` for records written
    /// before payload schema 5 and for projects never scanned.
    pub stack: Option<StackProfile>,
```

and to `ProjectRef` (after `last_seen`):

```rust
    /// Compact stack summary for the project cards. `None` for records written
    /// before payload schema 5.
    pub stack: Option<StackSummary>,
```

In `codec.rs`, set `pub const NATIVE_PAYLOAD_SCHEMA: u32 = 5;` and add beside `ACTOR_PAYLOAD_SCHEMA`:

```rust
/// The payload schema that introduced the deterministic stack profile.
const STACK_PAYLOAD_SCHEMA: u32 = 5;
```

Extend `encode_meta` / `decode_meta` to call the new helpers after the map helpers, and give `ProjectRef` the same schema parameter the other record kinds already receive (`encode_project_ref(writer, value, payload_schema)` / `decode_project_ref(reader, payload_schema)`; update the two call sites at `codec.rs:199` and `:227`).

```rust
fn encode_stack(
    writer: &mut Writer,
    value: Option<&StackProfile>,
    payload_schema: u32,
) -> Result<(), CodecError> {
    if payload_schema < STACK_PAYLOAD_SCHEMA {
        return if value.is_some() {
            Err(CodecError::NonCanonical)
        } else {
            Ok(())
        };
    }
    let Some(profile) = value else {
        return writer.bool(false);
    };
    if profile.projects.len() > MAX_STACK_PROJECTS {
        return Err(CodecError::ListTooLarge {
            actual: profile.projects.len(),
            maximum: MAX_STACK_PROJECTS,
        });
    }
    writer.bool(true)?;
    writer.u32(u32::try_from(profile.projects.len()).map_err(|_| CodecError::LengthOverflow)?)?;
    for project in &profile.projects {
        if project.evidence.len() > MAX_STACK_EVIDENCE {
            return Err(CodecError::ListTooLarge {
                actual: project.evidence.len(),
                maximum: MAX_STACK_EVIDENCE,
            });
        }
        writer.string(&project.root)?;
        writer.u8(project.language.wire_tag())?;
        writer.strings(&project.evidence)?;
        writer.u8(project.depth)?;
        writer.u32(project.files)?;
    }
    writer.string(&profile.scanned_head)?;
    writer.timestamp(profile.scanned_at)?;
    writer.u32(profile.tracked_files)?;
    writer.bool(profile.incomplete)
}

fn decode_stack(
    reader: &mut Reader<'_>,
    payload_schema: u32,
) -> Result<Option<StackProfile>, CodecError> {
    if payload_schema < STACK_PAYLOAD_SCHEMA || !reader.bool()? {
        return Ok(None);
    }
    let count = reader.u32()? as usize;
    if count > MAX_STACK_PROJECTS {
        return Err(CodecError::ListTooLarge {
            actual: count,
            maximum: MAX_STACK_PROJECTS,
        });
    }
    let mut projects = Vec::with_capacity(count);
    for _ in 0..count {
        let root = reader.string()?;
        let language = LanguageId::from_wire_tag(reader.u8()?).ok_or(CodecError::NonCanonical)?;
        let evidence = reader.strings()?;
        if evidence.len() > MAX_STACK_EVIDENCE {
            return Err(CodecError::ListTooLarge {
                actual: evidence.len(),
                maximum: MAX_STACK_EVIDENCE,
            });
        }
        projects.push(StackProject {
            root,
            language,
            evidence,
            depth: reader.u8()?,
            files: reader.u32()?,
        });
    }
    Ok(Some(StackProfile {
        projects,
        scanned_head: reader.string()?,
        scanned_at: reader.timestamp()?,
        tracked_files: reader.u32()?,
        incomplete: reader.bool()?,
    }))
}
```

Write `encode_stack_summary` / `decode_stack_summary` the same way: a `bool` presence flag, then a `u32` language count with a `u8` tag plus `u32` count per entry, then `tracked_files`, `scanned_head`, and `incomplete`. Cap the language list at `MAX_STACK_PROJECTS`. Below `STACK_PAYLOAD_SCHEMA`, a present summary is `CodecError::NonCanonical` and an absent one writes nothing.

The `reader.bool()?` guard must not run below schema 5 — the short-circuit `||` in `decode_stack` already ensures that, and the same ordering is required in the summary decoder.

- [ ] **Step 4: Fix every construction site**

`Meta` and `ProjectRef` are built in several places (`ptrack-store`, `ptrack-app`, tests). Add `stack: None` to each. Find them with:

```bash
cargo test --workspace --all-targets --no-run 2>&1 | grep -n "missing field \`stack\`" | head -40
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --package ptrack-core && cargo test --package ptrack-store`
Expected: PASS. The store suite must pass unchanged — the accepted payload-schema range is `MIN_NATIVE_PAYLOAD_SCHEMA..=NATIVE_PAYLOAD_SCHEMA`, so schema-4 records still validate on open.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add crates/ptrack-core crates/ptrack-store crates/ptrack-app crates/ptrack-cli
git commit -m "feat: persist the stack profile at payload schema 5"
```

---

### Task 3: Store read and write paths

**Files:**
- Modify: `crates/ptrack-store/src/project.rs` (near `meta()` at line 366 and `update_meta` at line 493)
- Modify: `crates/ptrack-store/src/global.rs` (`register_project` at line 173)
- Modify: `crates/ptrack-store/src/project_test.rs`, `crates/ptrack-store/src/store_test.rs` (whichever already covers `register_project`)

**Interfaces:**
- Produces: `ProjectStore::stack_profile(&self) -> StoreResult<Option<StackProfile>>`, `ProjectStore::set_stack_profile(&self, profile: StackProfile) -> StoreResult<()>`, `GlobalStore::set_project_stack(&self, path: &Path, summary: StackSummary) -> StoreResult<()>`.

- [ ] **Step 1: Write the failing test**

In `crates/ptrack-store/src/project_test.rs`:

```rust
#[test]
fn a_stack_profile_round_trips_through_meta() {
    let fixture = project_fixture();
    assert!(fixture.store.stack_profile().expect("read").is_none());
    let profile = StackProfile {
        projects: vec![StackProject {
            root: String::new(),
            language: LanguageId::Rust,
            evidence: vec!["Cargo.toml".to_owned()],
            depth: 0,
            files: 9,
        }],
        scanned_head: "deadbeef".to_owned(),
        scanned_at: Timestamp::Zero,
        tracked_files: 9,
        incomplete: false,
    };
    fixture.store.set_stack_profile(profile.clone()).expect("write");
    assert_eq!(fixture.store.stack_profile().expect("read"), Some(profile));
}
```

In the global-store test file:

```rust
#[test]
fn re_registering_a_project_preserves_its_stack_summary() {
    let fixture = global_fixture();
    fixture.store.register_project("ptrack", fixture.root()).expect("register");
    let summary = StackSummary {
        languages: vec![(LanguageId::Rust, 5)],
        tracked_files: 5,
        scanned_head: "deadbeef".to_owned(),
        incomplete: false,
    };
    fixture.store.set_project_stack(fixture.root(), summary.clone()).expect("set stack");
    fixture.store.register_project("ptrack", fixture.root()).expect("re-register");
    let stored = fixture.store.project(fixture.root()).expect("read").expect("present");
    assert_eq!(stored.stack, Some(summary));
}
```

Use each file's existing fixture helper names rather than inventing new ones.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --package ptrack-store stack`
Expected: FAIL — no method `stack_profile`.

- [ ] **Step 3: Implement the store methods**

In `project.rs`, beside `meta()`:

```rust
    /// Returns the stored stack profile, absent until the first scan.
    pub fn stack_profile(&self) -> StoreResult<Option<StackProfile>> {
        Ok(self.meta()?.stack)
    }

    /// Replaces the stored stack profile. The scan is the sole writer.
    pub fn set_stack_profile(&self, profile: StackProfile) -> StoreResult<()> {
        self.update_meta(|meta| meta.stack = Some(profile))
    }
```

In `global.rs`, make `register_project` carry the existing summary forward:

```rust
        let stack = self
            .active
            .store()
            .read(|tx| typed::get::<ProjectRef>(tx, RecordKey::Bytes(path.as_bytes())))?
            .and_then(|existing| existing.stack);
        let value = ProjectRef {
            name: name.into(),
            path: path.clone(),
            last_seen: self.clock.now_local(),
            stack,
        };
```

and add:

```rust
    /// Records the compact stack summary for one registered project. A project
    /// that is not registered is ignored rather than created.
    pub fn set_project_stack(
        &self,
        path: impl AsRef<Path>,
        summary: StackSummary,
    ) -> StoreResult<()> {
        let path = registry_path(path.as_ref())?;
        self.active.write(|tx| {
            let Some(mut value) =
                typed::get_write::<ProjectRef>(tx, RecordKey::Bytes(path.as_bytes()))?
            else {
                return Ok(());
            };
            value.stack = Some(summary);
            typed::put(tx, RecordKey::Bytes(path.as_bytes()), &value)?;
            Ok(())
        })
    }
```

Use whichever write-transaction getter the file already uses (`typed::get_write` or `required_write`) — match the neighbouring code exactly.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --package ptrack-store`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/ptrack-store
git commit -m "feat: store the stack profile and its registry summary"
```

---

### Task 4: Bounded tracked-path collector

**Files:**
- Create: `crates/ptrack-git/src/tracked.rs`, `crates/ptrack-git/src/tracked_test.rs`
- Modify: `crates/ptrack-git/src/runner.rs` (add a production constructor beside `for_test` at line 98)
- Modify: `crates/ptrack-git/src/lib.rs` (module and re-export)

**Interfaces:**
- Consumes: `ptrack_core::stack::resolve`, `RepositoryService`, `CancellationToken`, `RepositoryError`.
- Produces: `RepositoryService::capture_stack(&self, cancellation: &CancellationToken, root: &Path, head: &str, now: Timestamp) -> Result<StackProfile, RepositoryError>` and the constant `MAX_TRACKED_PATHS: usize = 200_000`.

- [ ] **Step 1: Write the failing test**

Create `crates/ptrack-git/src/tracked_test.rs`:

```rust
use std::sync::Arc;

use ptrack_core::{LanguageId, Timestamp};

use crate::runner::CancellationToken;
use crate::snapshot::RepositoryService;
use crate::tracked::MAX_TRACKED_PATHS;

#[test]
fn tracked_paths_resolve_into_a_profile_stamped_with_head() {
    let runner = fake_runner(b"Cargo.toml\0src/lib.rs\0frontend/package.json\0".to_vec());
    let profile = RepositoryService::with_runner(runner)
        .capture_stack(&CancellationToken::new(), std::path::Path::new("/repo"), "abc123", Timestamp::Zero)
        .expect("capture");
    assert_eq!(profile.tracked_files, 3);
    assert_eq!(profile.scanned_head, "abc123");
    assert!(!profile.incomplete);
    assert_eq!(profile.projects.len(), 2);
    assert_eq!(profile.projects[0].language, LanguageId::Rust);
}

#[test]
fn a_listing_over_the_cap_is_truncated_and_marked_incomplete() {
    let mut output = Vec::new();
    for index in 0..MAX_TRACKED_PATHS + 5 {
        output.extend_from_slice(format!("src/file{index:07}.rs\0").as_bytes());
    }
    output.extend_from_slice(b"Cargo.toml\0");
    let profile = RepositoryService::with_runner(fake_runner(output))
        .capture_stack(&CancellationToken::new(), std::path::Path::new("/repo"), "abc123", Timestamp::Zero)
        .expect("capture");
    assert!(profile.incomplete);
    assert_eq!(profile.tracked_files as usize, MAX_TRACKED_PATHS);
}

#[test]
fn an_empty_listing_yields_an_empty_profile() {
    let profile = RepositoryService::with_runner(fake_runner(Vec::new()))
        .capture_stack(&CancellationToken::new(), std::path::Path::new("/repo"), "abc123", Timestamp::Zero)
        .expect("capture");
    assert_eq!(profile.tracked_files, 0);
    assert!(profile.projects.is_empty());
}
```

Build `fake_runner` with the same fake-runner helper `snapshot_test.rs` already uses; import it rather than writing a second one if it is reachable, otherwise copy its shape.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --package ptrack-git tracked_test`
Expected: FAIL — unresolved module `crate::tracked`.

- [ ] **Step 3: Implement the collector**

Create `crates/ptrack-git/src/tracked.rs`:

```rust
//! Tracked-path collection for deterministic stack discovery.
//!
//! `git ls-files` is the single source of truth for project membership:
//! ignored, untracked, and vendored-but-ignored files never appear, so
//! `.gitignore` decides what counts without this crate reimplementing it.

use std::path::Path;

use ptrack_core::stack::resolve;
use ptrack_core::{StackProfile, Timestamp};

use crate::runner::{CancellationToken, RepositoryError, args};
use crate::snapshot::RepositoryService;

/// The most tracked paths one scan reads before reporting truncation.
pub const MAX_TRACKED_PATHS: usize = 200_000;

impl RepositoryService {
    /// Scans tracked paths and resolves them into a stack profile.
    ///
    /// # Errors
    ///
    /// Returns a content-free error when cancellation, a resource bound,
    /// subprocess execution, or decoding fails.
    pub fn capture_stack(
        &self,
        cancellation: &CancellationToken,
        root: &Path,
        head: &str,
        now: Timestamp,
    ) -> Result<StackProfile, RepositoryError> {
        let output = self.runner().output(
            cancellation,
            root,
            &args(["ls-files", "-z", "--deduplicate"]),
        )?;
        let mut paths: Vec<String> = output
            .split(|byte| *byte == 0)
            .filter(|entry| !entry.is_empty())
            .map(|entry| {
                std::str::from_utf8(entry)
                    .map(str::to_owned)
                    .map_err(|_| RepositoryError::InvalidData("tracked path is not UTF-8"))
            })
            .collect::<Result<_, _>>()?;
        paths.sort();
        let incomplete = paths.len() > MAX_TRACKED_PATHS;
        paths.truncate(MAX_TRACKED_PATHS);
        let tracked_files = u32::try_from(paths.len()).unwrap_or(u32::MAX);
        Ok(StackProfile {
            projects: resolve(&paths),
            scanned_head: head.to_owned(),
            scanned_at: now,
            tracked_files,
            incomplete,
        })
    }
}
```

`ls-files` output on a large repository exceeds the runner's 4 MiB default, and the scan is slower than the 3 s snapshot timeout allows. Add to `runner.rs`, beside `for_test`:

```rust
    /// Constructs a runner with explicit limits, for the tracked-path scan
    /// whose output is far larger than a snapshot's.
    pub(crate) fn with_limits(timeout: Duration, max_output_bytes: usize) -> Self {
        Self {
            git_path: OsString::from("git"),
            timeout,
            max_output_bytes,
            reader_counter: &ACTIVE_READER_THREADS,
            reader_limit: MAX_READER_THREADS,
        }
    }
```

and give `RepositoryService` a constructor that uses it plus a `runner()` accessor for `tracked.rs`:

```rust
    /// Constructs a service sized for tracked-path scans: a 32 MiB output
    /// ceiling and a 30 s timeout, both far above snapshot needs.
    #[must_use]
    pub fn for_stack_scan() -> Self {
        Self {
            runner: Arc::new(ExecRunner::with_limits(
                Duration::from_secs(30),
                32 * 1024 * 1024,
            )),
            now: unix_now,
        }
    }

    pub(crate) fn runner(&self) -> &dyn Runner {
        self.runner.as_ref()
    }
```

Add `mod tracked;` / `#[cfg(test)] mod tracked_test;` to `lib.rs` and re-export `MAX_TRACKED_PATHS`. If `with_runner` does not already exist as a test constructor on `RepositoryService`, reuse `with_runner_and_clock` (`snapshot.rs:211`) in the tests instead.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --package ptrack-git`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo clippy --package ptrack-git --all-targets -- -D warnings
git add crates/ptrack-git
git commit -m "feat: collect tracked paths for stack discovery"
```

---

### Task 5: Scan orchestration and the desktop command

**Files:**
- Modify: `crates/ptrack-app/src/desktop_runtime.rs:121` (command list), `:3897` (dispatch), `:5449-5498` (delete `RepoStatsView` and `repo_stats`)
- Modify: `crates/ptrack-app/src/desktop_runtime_test.rs`

**Interfaces:**
- Consumes: `RepositoryService::for_stack_scan`, `capture_stack`, `ProjectStore::{stack_profile, set_stack_profile}`, `GlobalStore::set_project_stack`, `ptrack_core::stack::summarize`.
- Produces: the command `GetStackProfileV1` returning
  `{ "state": "scanning" | "ready" | "unavailable" | "failed", "scannedHead": string, "scannedAt": string, "trackedFiles": number, "incomplete": boolean, "projects": [{ "root": string, "language": string, "files": number, "evidence": [string] }] }`,
  and `fn stack_scan_due(stored: Option<&StackProfile>, head: &str) -> bool`.
- Removes: `GetRepoStatsV1`, `RepoStatsView`, `repo_stats`.

- [ ] **Step 1: Write the failing test**

In `crates/ptrack-app/src/desktop_runtime_test.rs`:

```rust
#[test]
fn a_scan_is_due_when_no_profile_is_stored() {
    assert!(stack_scan_due(None, "abc123"));
}

#[test]
fn a_scan_is_due_when_head_moved() {
    let stored = StackProfile { scanned_head: "abc123".to_owned(), ..StackProfile::default() };
    assert!(stack_scan_due(Some(&stored), "def456"));
    assert!(!stack_scan_due(Some(&stored), "abc123"));
}

#[test]
fn a_truncated_profile_is_not_rescanned_on_head_movement() {
    let stored = StackProfile {
        scanned_head: "abc123".to_owned(),
        incomplete: true,
        ..StackProfile::default()
    };
    assert!(!stack_scan_due(Some(&stored), "def456"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --package ptrack-app stack_scan_due`
Expected: FAIL — `stack_scan_due` not found.

- [ ] **Step 3: Implement the cadence rule and the command**

In `desktop_runtime.rs`, replacing the deleted `repo_stats` block:

```rust
/// Reports whether a tracked-path scan is due.
///
/// A truncated profile is exempt from HEAD-driven rescans: the listing was
/// already over the cap, so re-reading it on every commit costs far more than
/// the staleness it would remove. Project open and explicit rescan still run.
pub(super) fn stack_scan_due(stored: Option<&StackProfile>, head: &str) -> bool {
    match stored {
        None => true,
        Some(profile) if profile.incomplete => false,
        Some(profile) => profile.scanned_head != head,
    }
}
```

The command handler:

1. Reads the current HEAD from the snapshot the runtime already holds. No HEAD (an empty repository, or not a repository) returns `{"state": "unavailable"}` and writes nothing.
2. Reads the stored profile through `ProjectStore::stack_profile`.
3. When `stack_scan_due` is false, serves the stored profile as `ready`.
4. When it is true, runs `RepositoryService::for_stack_scan().capture_stack(...)`. On success it writes the profile with `set_stack_profile`, writes `summarize(&profile)` with `set_project_stack`, and returns `ready`. On error it returns `{"state": "failed"}` **and leaves the stored profile untouched** — no partial write, no cleared counts.
5. A `force` argument skips step 3 and always scans. The desktop sends it on project open and from the panel's rescan control — that is how a truncated profile, exempt from HEAD-driven rescans, still refreshes.

Register `"GetStackProfileV1"` in the command list at line 121 and add the dispatch arm beside the others at line 3897. Delete `"GetRepoStatsV1"` from both, and delete `RepoStatsView` and `repo_stats` entirely.

- [ ] **Step 4: Add the failure and persistence test**

```rust
#[test]
fn a_failed_scan_preserves_the_stored_profile() {
    // Build a runtime whose stack service fails, with a profile already
    // stored, then assert the stored profile is unchanged and the view
    // reports "failed". Follow the fixture pattern used by the neighbouring
    // desktop runtime tests.
}
```

Fill this in against the fixture helpers already in `desktop_runtime_test.rs`; the assertion is that `stack_profile()` returns the pre-existing profile and the command result's `state` is `"failed"`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --package ptrack-app`
Expected: PASS, and no reference to `repo_stats` remains: `grep -rn "repo_stats\|RepoStats" crates/` returns nothing.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
cargo clippy --package ptrack-app --all-targets -- -D warnings
git add crates/ptrack-app
git commit -m "feat: scan the stack on open and on HEAD movement"
```

---

### Task 6: Desktop surfaces

**Files:**
- Modify: `frontend/src/app.js:1088-1092` (Overview tiles), `:1528-1540` (`loadRepoStats`), `:2755`, `:6544-6545` (reset), `:684-685` (state)
- Modify: `frontend/src/tauri-bridge.js:56` (command allowlist), `frontend/src/tauri-bridge.test.js:20`
- Modify: `frontend/src/workspace/recent-projects.ts` (entry shape and parser)
- Modify: `frontend/src/workspace/recent-projects.test.ts`

**Interfaces:**
- Consumes: `GetStackProfileV1` from Task 5, and the `stack` field now present on each recent-project entry.
- Produces: `RecentProjectEntry.stack?: { languages: { language: string; files: number }[]; trackedFiles: number }`.

- [ ] **Step 1: Write the failing frontend test**

In `frontend/src/workspace/recent-projects.test.ts`:

```ts
it("parses a stack summary when the entry carries one", () => {
  const [entry] = parseRecentProjects({
    projects: [
      {
        ...validEntry,
        stack: { languages: [{ language: "rust", files: 214 }], trackedFiles: 252 },
      },
    ],
  });
  expect(entry.stack).toEqual({
    languages: [{ language: "rust", files: 214 }],
    trackedFiles: 252,
  });
});

it("accepts an entry with no stack summary", () => {
  const [entry] = parseRecentProjects({ projects: [validEntry] });
  expect(entry.stack).toBeUndefined();
});

it("rejects a stack summary with a negative file count", () => {
  expect(() =>
    parseRecentProjects({
      projects: [
        {
          ...validEntry,
          stack: { languages: [{ language: "rust", files: -1 }], trackedFiles: 1 },
        },
      ],
    }),
  ).toThrow();
});
```

Reuse the file's existing valid-entry fixture rather than adding a new one.

- [ ] **Step 2: Run test to verify it fails**

Run: `cd frontend && npx vitest run src/workspace/recent-projects.test.ts`
Expected: FAIL — `entry.stack` is undefined for the first case.

- [ ] **Step 3: Parse and render**

Extend `RecentProjectEntry` and `parseRecentProjects` with an optional, strictly validated `stack`: `languages` at most 16 entries, each `language` a non-empty string under 64 characters, each `files` a non-negative safe integer, `trackedFiles` likewise. Anything malformed throws, matching the file's existing style.

Render the label on the card: the first two languages, each with its count, e.g. `rust 214 · typescript 38`. No percentages.

- [ ] **Step 4: Replace the Overview tiles**

In `app.js`, rename the state pair at line 684 to `stackProfileRequested` / `stackProfile`, replace `loadRepoStats` with `loadStackProfile` calling `api().GetStackProfileV1()`, and replace the tile block at line 1088:

```js
  if (stackProfile?.state === "ready") {
    counts.append(statElement(stackProfile.trackedFiles.toLocaleString(), "Tracked files"));
    stackProfile.projects.slice(0, 4).forEach((project) => {
      counts.append(
        statElement(project.files.toLocaleString(), languageLabel(project.language)),
      );
    });
    if (stackProfile.incomplete) {
      counts.append(statElement("partial", "Scan truncated"));
    }
  }
```

`languageLabel` is a fixed lookup beside the other presentation helpers,
mapping each identifier the backend can send to its display name — `rust` to
`Rust`, `typescript` to `TypeScript`, `csharp` to `C#`, `php` to `PHP`,
`c` to `C/C++`, `container` to `Containers`, and the rest capitalized — falling
back to the raw identifier for anything unrecognized so a newer backend never
renders an empty label.

The `Lines of code` tile is deleted, not reworded. Update the reset block at line 6544 and the forced refresh at line 2755 to the new names, and swap `GetRepoStatsV1` for `GetStackProfileV1` in `tauri-bridge.js` and its test.

- [ ] **Step 5: Repository panel and open-journey step**

Render the panel section from the same `GetStackProfileV1` result: `scanning` shows `Scanning…`; `ready` lists each discovered project as root, language, file count, and its evidence paths, above a `scanned at <short sha> · <relative time>` line and a `Rescan` button calling the command with `force`; `failed` shows the failure with a `Retry` button; `unavailable` states the project is not a git repository. Show a truncation note when `incomplete` is true. The project-open journey gains one step reading the same states, so a failed scan stays visible after the journey completes.

- [ ] **Step 6: Run tests to verify they pass**

Run: `cd frontend && npm test`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add frontend
git commit -m "feat: show the discovered stack instead of a line count"
```

---

### Task 7: Agent context digest

**Files:**
- Modify: `crates/ptrack-cli/src/dispatch.rs:1446` (`context_command`)
- Modify: `crates/ptrack-cli/src/dispatch_test.rs`
- Modify: `crates/ptrack-cli/src/compat_json.rs` and `compat_json_test.rs` if the JSON view is assembled there

**Interfaces:**
- Consumes: `ProjectStore::stack_profile`.
- Produces: a `Stack` section in the display digest and a `stack` object in the JSON view.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn the_context_digest_names_the_discovered_stack() {
    let fixture = cli_fixture();
    fixture.store.set_stack_profile(StackProfile {
        projects: vec![
            StackProject { root: String::new(), language: LanguageId::Rust, evidence: vec!["Cargo.toml".to_owned()], depth: 0, files: 214 },
            StackProject { root: "frontend".to_owned(), language: LanguageId::TypeScript, evidence: vec!["frontend/package.json".to_owned()], depth: 1, files: 38 },
        ],
        scanned_head: "abc123".to_owned(),
        scanned_at: Timestamp::Zero,
        tracked_files: 252,
        incomplete: false,
    }).expect("write");

    let output = fixture.run(&["context"]).expect("context");
    assert!(output.contains("Stack"));
    assert!(output.contains("rust"));
    assert!(output.contains("214"));
    assert!(output.contains("frontend"));
}

#[test]
fn a_project_with_no_scan_omits_the_stack_section() {
    let fixture = cli_fixture();
    let output = fixture.run(&["context"]).expect("context");
    assert!(!output.contains("Stack"));
}
```

Use the dispatch test file's own fixture helpers and its assertion style.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --package ptrack-cli context`
Expected: FAIL — no `Stack` section.

- [ ] **Step 3: Emit the section**

In `context_command`, after the existing sections, read `stack_profile()`. When present and non-empty, print one line per discovered project: root (or `.` at the repository root), language, and file count, plus a truncation note when `incomplete`. Absent or empty profiles print nothing. The JSON view carries the same values as structured fields — never a formatted string a consumer must parse.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --package ptrack-cli`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/ptrack-cli
git commit -m "feat: carry the discovered stack in the context digest"
```

---

### Task 8: Documentation, help gate, and full local gate

**Files:**
- Modify: `CHANGELOG.md`
- Modify: `README.md` (the desktop workspace section, if it describes the Overview tiles)
- Modify: `docs/help/assets/screenshots/manifest.json` (`uiSourceSha256`)

- [ ] **Step 1: Add the changelog entry**

Under the unreleased heading, in the file's existing style: the Overview now reports discovered languages and tracked-file counts from a deterministic manifest scan; the line-count tile is gone; the stack appears in the Repository panel, on recent-project cards, and in `ptrack context`.

- [ ] **Step 2: Refresh the screenshot manifest digest**

`tools/help_check.py all` hashes the UI sources listed in the manifest and fails with `screenshot manifest: UI sources changed` after any edit to `frontend/src/app.js`. Recompute with `source_digest()` from `tools/help_check.py` and write it back to `uiSourceSha256`.

```bash
python3 -B -c "import sys; sys.path.insert(0, 'tools'); import help_check; print(help_check.source_digest())"
python3 -B tools/help_check.py all
```

- [ ] **Step 3: Flag the stale screenshots**

The Overview tile row changed, so the checked-in Overview screenshots no longer match the app. Screenshot capture is manual — do not fake it. Report to the owner, in the PR description, exactly which screenshots need recapture.

- [ ] **Step 4: Run the full gate**

Run: `make test`
Expected: every stage passes. Do not start it while another `cargo` or `rustc` process is running on the host — check first with `ps` and wait until it is quiet.

- [ ] **Step 5: Commit and open the PR**

```bash
git add CHANGELOG.md README.md docs/help/assets/screenshots/manifest.json
git commit -m "docs: record deterministic stack discovery"
git push -u origin feat/stack-discovery
gh pr create --title "feat: deterministic stack discovery" --body "..."
```

The PR body names the removed `Lines of code` tile, the payload schema bump to 5 (a database written by this build is not readable by an older ptrack), and the screenshots needing manual recapture.
