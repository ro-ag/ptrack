# ptrack-app

`ptrack-app` is the UI-neutral application boundary shared by the CLI, TUI,
and Tauri adapters. Its configuration contains explicit canonical project and
global database paths, exact `ActiveBinding` values, and the writer version.
Each service call reopens and verifies the required store and drops it before
returning, so an idle UI cannot retain the database writer lock.

## Guide publication hardening

Guide installation treats project instruction files and the global guide
template as untrusted filesystem entries. It refuses symbolic links and
special files. It verifies opened files and parent directory handles against
their captured filesystem identities, preserves an existing guide's mode, and
writes changes to a create-new sibling temporary file. The destination identity
is rechecked before atomic publication, and the parent directory is synchronized.
Temporary files are removed on publication failure. The pure guide body,
rendering, and marker upsert live in `ptrack-core`, where they can be tested
without filesystem authority.

Unix publication and removal are descriptor-relative (`openat`, `renameat`,
`statat`, and `unlinkat`) against the pinned parent handle, so a concurrent
parent namespace swap cannot redirect the write. Other platforms fail closed
until an equivalent descriptor-relative implementation is available.
