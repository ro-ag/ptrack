# ptrack-store

Versioned, pure-Rust transactional storage for the ptrack rewrite.

The crate wraps `redb` and deliberately keeps its database and transaction
handles private. The ptrack storage or migration executable owns real database
paths and all writes. Tests create isolated files under the operating system's
temporary directory; they do not discover or open user databases.

Safety boundaries:

- `create_new` refuses every existing destination and all symbolic links.
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
- Unix database files are created as `0600` and insecure existing modes fail
  closed.

The one-way bbolt exporter/importer and the native Rust model codecs are
separate plan items. They must target a distinct `.redb` file and keep the Go
source open read-only.
