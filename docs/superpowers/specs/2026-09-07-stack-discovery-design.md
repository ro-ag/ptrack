# Deterministic stack discovery

## Outcome

p-track states what a project is built from by naming the manifests it found,
not by weighing file sizes or line counts. Opening a project scans its tracked
files once, resolves the discovered subprojects and their languages, counts the
tracked files behind each one, and stores that profile. The desktop shows the
profile with the evidence that produced it, recent-project cards carry a stack
label before the project is opened, and the agent context digest carries the
same structured answer so a fresh agent does not have to guess the stack.

Size-weighted language ratios are misleading in a mixed repository: a vendored
directory, a generated bundle, or one large fixture file can outweigh the code
that defines the project. This design removes that class of answer entirely.

## Determinism contract

- The resolver is a pure function from a sorted list of repository-relative
  tracked paths to a profile. It performs no filesystem access, runs no
  subprocess, and reads no clock.
- The same path list always produces a byte-identical profile, independent of
  the order in which the collector observed the paths.
- Language identity comes from a fixed, ordered marker table compiled into the
  binary. A language is discovered because a specific manifest path is tracked,
  never because an extension was common in the tree.
- Ranking is total and has no ties: shallowest project root first, then marker
  table order, then lexicographic path order.
- Counts are integers. Bytes and lines are never collected, stored, or
  displayed. No percentage is persisted; a surface that renders a proportion
  derives it and must display the underlying count alongside it.

## Discovery model

- **Collection.** `git ls-files -z --deduplicate` at the repository root
  produces the tracked path set. Ignored, untracked, and vendored-but-ignored
  files are absent by construction, so `.gitignore` is the single source of
  truth for what belongs to the project.
- **Markers.** Each table entry maps a manifest filename or extension to one
  language: `Cargo.toml`, `go.mod`, `package.json`, `tsconfig.json`,
  `pyproject.toml`, `setup.py`, `Package.swift`, `pom.xml`, `build.gradle`,
  `build.gradle.kts`, `*.csproj`, `Gemfile`, `composer.json`, `mix.exs`,
  `pubspec.yaml`, `CMakeLists.txt`, `*.tf`, `Dockerfile`.
- **Subprojects.** Every tracked manifest defines a discovered project rooted
  at its directory. Workspace membership is not expanded from manifest
  contents: a Cargo workspace member, an npm workspace package, or a `go.work`
  module appears because its own manifest is tracked. This keeps the resolver a
  pure function of the path list and avoids parsing manifest globs.
- **Refinement.** A JavaScript project is reported as TypeScript when a
  `tsconfig.json` is tracked in the same directory, or when any tracked file
  attributed to it is a `.ts`/`.tsx` source. Declaration files (`.d.ts`) do not
  refine: they describe JavaScript rather than prove the project is written in
  TypeScript. Refinement never invents evidence — an untracked `tsconfig.json`
  is not listed as a manifest.
- **Attribution.** Every tracked file is attributed to the nearest enclosing
  discovered project and counted by extension. Files under no discovered
  project are counted once at the repository level as unattributed.
- **Evidence.** Each discovered project records the manifest paths that
  produced it, so the desktop can answer why a language was reported and a
  wrong answer is falsifiable by inspection.

## Records

Persistence is additive at the payload-schema level only. No collection is
added and `STORE_SCHEMA_VERSION` does not move: the database validator demands
an exact table catalog and an exact schema version, and no in-place upgrade
path exists, so a new collection would refuse to open every database written by
an earlier build. Both new fields follow the mechanism that introduced the
per-actor maps at payload schema 3 — written only at or above the schema that
defines them, absent and empty when decoded from an older record.

- The project database stores the full profile as an additive `Meta.stack`
  field: the discovered projects, their languages, evidence paths, depth, and
  per-project file counts, plus the HEAD sha the scan ran against, the scan
  timestamp, the total tracked file count, and a truncation flag.
- The global database carries an additive stack summary on `ProjectRef`: the
  ranked language identifiers with their file counts, and the total tracked
  file count. Recent-project cards read only this summary and never open a
  project database to render a list.
- A record written before this feature decodes with an absent profile or
  summary. That is not an error: the panel scans, and the card renders without
  a label.

## Scan cadence

- Opening a project scans it, whether it was just initialized or has been
  opened before.
- Every later repository capture compares the stored scan sha against current
  HEAD. Any difference triggers a full rescan and recount. Deltas are never
  derived from a diff, so a rebase, an amend, a force-push, or a branch switch
  cannot leave the counts drifting.
- An identical HEAD performs no scan and no store write.
- A repository whose tracked path listing exceeds 200,000 entries is recorded
  as truncated and drops to a cheaper cadence: scans occur on project open and
  on explicit rescan only, never on HEAD movement.
- Stack collection is separate from `RepositoryService::capture`. The bounded
  snapshot keeps its existing command budget and byte ceiling; scanning has its
  own entry point and its own cadence.

## Failure and absence

- A project root that is not a git repository has no profile. The Repository
  panel states that plainly. There is no filesystem-walk fallback.
- A failed scan leaves the previous profile intact, records the failure for the
  surface to display, and offers a retry. A failure never blocks project open,
  never clears stored counts, and never writes a partial profile.
- A cancelled scan is treated as a failure with no store write.

## Surfaces

- **Overview tiles.** The `Lines of code` tile is removed together with the
  line-counting `repo_stats` path behind it. `Tracked files` remains and is
  served by the scan. A language breakdown replaces the removed tile, showing
  each discovered language with its tracked-file count.
- **Repository panel.** Shows `Scanning…` while a scan runs, then the
  discovered projects with language, file count, and evidence paths, above a
  persistent line naming the short sha and time of the scan. A rescan control
  is always available. Truncated profiles say so.
- **Project open journey.** The scan appears as a step with an explicit
  completed or failed state that remains readable after the journey finishes.
- **Recent-project cards.** Render the stored summary as a stack label. A
  project never opened by a build carrying this feature shows no label rather
  than a guess.
- **Agent context.** `ptrack context` emits the discovered projects, their
  languages, and their counts in both display and JSON output, so a resuming
  agent receives the structure of the workspace rather than inferring it.

## Testing

- Resolver: fixture path lists per language and per workspace shape, compared
  against golden profiles. A determinism test feeds each fixture shuffled and
  asserts identical output.
- Collector: injected runner output covering the cap boundary, the truncation
  flag, and a repository with no tracked files.
- Store: profile round-trip, and decoding a `ProjectRef` written at the
  previous payload schema yields an absent summary instead of an error.
- Orchestration: HEAD drift rescans, identical HEAD does not, a failed scan
  preserves the prior profile, and a truncated profile suppresses HEAD-driven
  rescans.
- Frontend: panel rendering for scanning, resolved, truncated, not-a-repository,
  and failed states, plus card label rendering with and without a summary.

## Out of scope

- Line, byte, or token counts in any form.
- Manifest content parsing, including workspace member globs and dependency
  lists.
- Language detection for untracked or ignored files.
- Per-commit incremental counting.
