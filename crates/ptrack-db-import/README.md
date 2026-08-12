# ptrack-db-import

`ptrack-db-import` is a standalone, create-only JSON-to-redb migration tool. It
never opens a bbolt database and never activates or replaces a database.

```console
ptrack-db-import \
  --manifest /absolute/private-stage/manifest.json \
  --destination /absolute/absent-candidate-directory \
  --accept-all
```

The command validates the complete immutable stage before inspecting the
destination. It then creates schema-v3 `.redb` candidates, closes and reopens
each candidate, verifies its provenance, and writes `receipt.json` only after
the whole batch is ready.

Legacy sources are never opened or modified, candidates remain inactive, and
the command performs no cutover, activation, or backup copy. Valid legacy
capabilities are disabled and require reapproval; malformed capability history
is retained only in the store's private inert quarantine.

The destination must not exist. During creation it contains `incomplete.json`.
If the command fails, that marker and any already-ready candidates remain for
inspection; there is no receipt and nothing is activated. Remove that new
candidate directory explicitly before retrying the same destination path.
The current command is Unix-only because its no-clobber guarantees depend on
pinning filesystem device/inode identities.
