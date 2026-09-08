# Insights — design

Status: proposed
Date: 2026-09-08

## Purpose

A fourth workspace destination, beside Overview, Board and Issues, that answers
questions the Overview cannot: how the project has moved over time, where the
work sits, and what it is made of. The Overview is a status panel read in five
seconds; Insights is read when someone wants to understand a trend.

## What the data honestly supports

The store keeps `created_at` and `updated_at` on every record plus the current
status. There is no status-change journal, so the following distinction governs
every chart on the page.

**Exact.** Notes and commits by day; tasks, plans and issues by creation date;
open-issue age; issue severity mix; per-plan progress; tracked files and lines
per language; the git commit history and tags read from the repository.

**Approximate, and labelled as such in the interface.** "Completed over time"
can only be dated by `updated_at` on records whose current status is done, so a
later edit moves the point. The same applies to closed issues and to lead time
(created to completed). These are shown with their basis named — never as
velocity or as a guarantee.

**Out of scope.** True burndown, time-in-status and duration tracking need a
status journal or a timer record. Both are schema changes, and the schema
immutability rule (project note #374) makes that a separate, deliberate piece of
work rather than something added here.

Two data sources are distinct and must not be conflated. `Commit` records in the
p-track store exist only for commits made after `ptrack hook install` — this
repository has 7 of them against hundreds in git. Anything describing project
history reads the repository through `ptrack-git`; the store's commit records
are used only where task linkage matters.

## Backend

### `GetInsightsV1 { weeks }`

A read-only aggregation beside `GetActivityHeatmapV2` in `desktop_runtime.rs`,
computed from the existing `ProjectSnapshot`. Bounded at 52 weeks like the
heatmap, with the same local-calendar-day bucketing (`heatmap_at` already
handles the offset correctly and its behaviour is pinned by a test).

Returns: daily counts split by kind; weekly cumulative created against
completed; lead-time buckets; per-plan progress; issues by severity and by age;
weekday-by-hour throughput; and the stored stack profile per language.

Nothing new is persisted. There is no format version to change.

### `GetProjectTimelineV1 { weeks }`

Repository history for the timeline, read through the existing bounded git
runner: commit timestamps over the window, and tags with their creation dates
for release markers. Both respect the aggregate byte cap, the cancellation
token and the runner abstraction that the snapshot capture already uses, so the
existing fake-runner tests extend to cover them.

### Test-file counting

The stack scan already walks tracked paths per project. It gains a count of
paths matching test conventions (`*_test.rs`, `*.test.ts`, `*_test.go`,
`tests/`). Reported as "test files", never as "tests" — it counts files, not
cases.

## Frontend

`#insights-page`, `view` gains `"insights"`, a nav item with its own accelerator,
and lazy loading on first visit in the same shape as the heatmap and stack
profile. Pure aggregation and shaping helpers live in
`frontend/src/workspace/insights.ts` and are unit-tested; only drawing lives in
`app.js`.

### The charts

Each answers a question no other one answers.

1. **Project timeline** — the page's centrepiece. Commit density across the
   project's life from git, with release tags marked, plan spans beneath, and
   milestone dates.
2. **Burn-up** — cumulative tasks created against completed.
3. **Throughput punchcard** — weekday by hour, showing when the project is
   actually worked.
4. **Lead time** — distribution from task creation to completion.
5. **Plan progress** — one bar per plan, sorted by completion.
6. **Issue mix** — severity split with an open-age strip.
7. **Composition** — files, lines and test files per language.

### Technology

`d3-scale`, `d3-shape`, `d3-array` and `d3-time` (about 60 KB) for scales,
curves and stacking. Drawing and styling stay hand-written so the charts
inherit the ring's gradient language and the app's hairlines rather than a
library's default look. TanStack was considered and does not apply: its charting
package is React-only and this frontend has no React.

Animation uses the Web Animations API and SVG path-length drawing: one
orchestrated entrance per chart on first reveal, motion on interaction, nothing
looping. `prefers-reduced-motion` renders the final state immediately.

## Testing

Rust unit tests for every aggregation, including empty projects, single-day
windows, the week bound, and local-day bucketing across a DST boundary. Git
queries are covered through the existing fake runner. Frontend helpers are
tested under vitest. The Help Center gains a page for the section, and the
screenshot manifest digest is refreshed.

## Release

v0.38.0 stays untagged until this lands, by the owner's decision. The changelog
section for 0.38.0 is rewritten to cover the Overview work, the rolling-summary
bound and Insights together before the tag is pushed.
