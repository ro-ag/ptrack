# p-track Help Center architecture

This directory is the source for the static p-track Help Center published at
`https://ro-ag.github.io/ptrack/help/`. GitHub Pages serves the repository's
`docs/` directory, so every published link must work below the project-site
prefix `/ptrack/` and when `docs/` is served locally.

## Information architecture

| Route | Purpose |
| --- | --- |
| `help/` | Orientation, featured guides, and Help Center search |
| `help/start-here/` | First project, plan, task, and resume workflow |
| `help/desktop/` | Board, drawer, Overview, navigation, and project switching |
| `help/terminals/` | Sessions, tabs, splits, safety, profiles, and recovery |
| `help/agents-and-capabilities/` | Agents, associations, handoffs, worktrees, and drift |
| `help/reference/` | CLI, TUI, desktop shortcut, and project-search reference |
| `help/reference/shortcuts/` | Stable native Help-menu destination for keyboard shortcuts |
| `help/install-and-safety/` | Installation, updates, storage, privacy, backup, and migration |
| `help/troubleshooting/` | Diagnostics, recovery, and frequently asked questions |

The top-level navigation exposes Start Here, Desktop, Terminals, Agents,
Reference, Install & Safety, and Troubleshooting in that order.
The same order is used in the no-JavaScript sitemap and the search scope.

## URL policy

- Published routes are unversioned, stable, lowercase directory URLs with a
  trailing slash. They always describe the current stable release.
- Links between Help pages and assets are relative. Root-absolute links such
  as `/help/` and `/assets/` are forbidden because they break the GitHub Pages
  project-site prefix.
- The application opens only fixed symbolic destinations implemented in its
  native backend. Frontend, project, agent, and terminal input never supplies
  an external Help URL.
- Content for an older release remains available through that release's tag on
  GitHub rather than by duplicating versioned pages in this tree.

## Search contract

Search is a small client-side enhancement over the full navigation. The
checked-in `search-index.json` contains only public Help copy: title, summary,
headings, keywords, and relative URL. Search is case-insensitive, runs locally,
and never sends a query off-device. Every result is also reachable through the
navigation when JavaScript is unavailable.

## Version contract

`site.json` records the Help Center schema and the stable product version. The
version must match the newest released heading in `CHANGELOG.md`, the Tauri
and CLI Cargo versions, the README release badge, the visible Help
Center release banner, the search index, and the screenshot manifest. Local
validation enforces this contract.

## Content and accessibility contract

- Source code and tests are authoritative when older prose disagrees.
- Pages use one descriptive `h1`, ordered headings, a skip link, a labeled
  primary navigation, useful link text, and meaningful alternative text.
- The site supports keyboard-only use, 200% zoom, both color schemes, and the
  user's reduced-motion preference.
- Security and recovery limitations are stated where a feature is introduced,
  not hidden in a generic disclaimer.
- Screenshots use sanitized fixture data only and are tracked by a manifest;
  live repository names, paths, terminals, credentials, and agent state must
  never appear in published assets.
