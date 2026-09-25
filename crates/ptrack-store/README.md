# ptrack-store

`ptrack-store` owns the project and global redb databases. `ProjectStore` and
`GlobalStore` expose typed operations through an exact active-runtime binding.
Closure errors abort record changes and ID allocation. A path identity failure
after a committed write is reported separately so callers do not retry it.

The current application schema is v4. Opening a database validates its kind,
manifest, exact table set, and stored records before writable use; it does
not migrate the database. Native payload schemas 1 through 9 remain readable
and upgrade one record at a time when written. New project data belongs in an
existing record at the next payload schema: adding a table would make existing
databases fail the exact-catalog check.

New database creation refuses existing paths, symlinks, and the legacy bbolt
filenames. Project operations retain and recheck the canonical root, private
`.ptrack` directory, database path, and activation binding. Runtime processes
hold a shared cutover lease; offline activation or rollback requires the
exclusive lease. A separate bootstrap lease serializes project additions and
retirements that preserve the live generation.

Database files are private to the current user. On Unix, creation uses mode
`0600`; an existing file with leaked group or other permission bits is
tightened and verified on open. Windows checks the file's private ACL. A
database that cannot meet the private-file policy is refused.
