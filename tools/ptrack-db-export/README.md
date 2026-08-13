# ptrack-db-export

`ptrack-db-export` is the standalone, read-only half of the database migration.
It freezes the global bbolt database and every project in its registry, then
writes a private JSON stage without modifying or copying the source databases.

```console
cd tools/ptrack-db-export
go run . \
  --home /absolute/path/to/.ptrack \
  --output /absolute/private-parent/absent-stage
```

The output parent must already exist and be private to the current user. The
stage is create-only, uses mode `0700` with `0600` artifacts on Unix and a
protected current-user-only DACL with reparse rejection on Windows, and
publishes `manifest.json` last. Any malformed non-capability record aborts the
export; malformed legacy capabilities and audits are preserved as inert
quarantine.

This command performs no import, backup copy, activation, replacement, app
installation, or automatic startup migration. Pass the resulting
`manifest.json` explicitly to `ptrack-db-import` to create inactive redb
candidates. This nested module is the only Go code retained after cutover; it
contains frozen legacy decode structs and no application write path.
