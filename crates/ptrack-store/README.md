# ptrack-store

Versioned, pure-Rust transactional storage for the ptrack rewrite.

The crate wraps `redb` and deliberately keeps its database and transaction
handles private. The ptrack storage or migration executable owns real database
paths and all writes. Tests create isolated files under the operating system's
temporary directory; they do not discover or open user databases.

Safety boundaries:

- `create_new` refuses every existing destination and all symbolic links.
- Unix create/import operations pin the original non-symlink parent directory,
  fence its device/inode identity around absence checks, file creation, every
  parent sync, and successful return, and verify the created file's identity.
  Creation remains path-relative rather than descriptor-relative: a detected
  parent swap immediately after `create_new(2)` can retain a private empty file
  in the moved directory, but no redb content is written and redirected work is
  never reported as successful. Non-Unix creation currently fails closed
  because stable safe `std` cannot provide equivalent directory identity.
- Once it creates a destination entry, a later initialization error leaves the
  partial file in place for explicit tool-owned recovery; it never unlinks a
  pathname that another process could have replaced.
- The legacy bbolt names `ptrack.db` and `global.db` are always forbidden.
- `open_existing` snapshots a locked descriptor and runs redb validation or
  repair only against that in-memory copy before upgrading the same descriptor
  to a writer. It never upgrades the ptrack application schema.
- Every successful write uses immediate durability and quick-repair metadata.
- Closure errors and panics abort both record changes and sequence allocation.
- Project and global collections, key representations, and sequence ownership
  are closed enums checked by the wrapper.
- Schema v3 manifests have exact origin-specific key sets. Standalone imports
  use `json-stage` origin and atomically commit stage version, source format,
  batch-manifest SHA-256, per-database JSON SHA-256, quarantine count, and the
  `ready` state. Schema v1, v2, and newer files are rejected without upgrade.
- Imports accept canonical native codec/schema payloads for every modeled
  collection. Only global config and backup values retain the validated raw
  codec. Every native value is decoded, validated, canonically re-encoded, and
  bound to its collection key before creation, ordinary writes, and reopen.
- Invalid legacy capabilities and audits may be preserved only in the private
  `ptrack.migration.quarantine` table. Its closed reason, source bucket, exact
  source key, exact gob value, and SHA-256 are verified before creation and on
  reopen. Ordinary collection APIs cannot address quarantine data.
- Import parent identity is rechecked inside the transaction immediately before
  committing provenance plus `ready`. A namespace change after that commit is
  reported distinctly as a committed-path change; it is never described as an
  incomplete `importing` database. A final transaction commit error is reported
  as outcome-unknown rather than as a definitely incomplete import.
- Unix database files are created as `0600` and insecure existing modes fail
  closed.

The standalone bbolt-to-JSON exporter and JSON-to-redb importer live outside
the application. They target distinct staging and `.redb` paths, keep Go
sources read-only, translate modeled records to the native positional codec,
and revoke or quarantine legacy authority. Application cutover remains a
separate explicit operation.
