# ptrack-migrate

`ptrack-migrate` validates and explicitly imports the private, one-way
interchange file produced by the Go bbolt exporter:

```text
ptrack-migrate inspect --bundle ABSOLUTE_PATH
ptrack-migrate import --bundle ABSOLUTE_PATH --destination ABSOLUTE_ABSENT.redb --accept-one-way
```

Import validates the complete bundle before creating a destination, preserves
the source's opaque bytes and exact sequences in typed `ptrack-store` records,
and verifies the new database before reporting success. It never discovers
databases, replaces a path, selects a default, installs an application, or
performs cutover.

Migration currently fails closed on Windows. The Go exporter does not yet
create private Windows ACLs, and the Rust validator does not accept a bundle
without stable file-identity verification. Imported Go gob payloads remain
opaque until the native Rust model codecs are implemented.

## PTRKMIG1 version 1

All integers are unsigned and big-endian. The 40-byte header is:

```text
magic[8]="PTRKMIG1" | version:u16=1 | header_len:u16=40
kind:u8 (1=project, 2=global) | flags:u8=0 | reserved:u16=0
source_format:u64 | bucket_count:u32 | reserved:u32=0 | total_records:u64
```

Each canonical bucket is a `BUKT` section, in strictly increasing raw-byte
lexical order:

```text
"BUKT" | name_len:u16 | flags:u16=0 | sequence:u64 | record_count:u64
name[name_len]
repeat record_count: key_len:u64 | value_len:u64 | key | value
```

Keys must be nonempty and strictly increasing as raw bytes. Numeric collection
keys are nonzero eight-byte integers and their bucket sequence cannot be below
the maximum key. Project `meta` contains exactly the `meta` singleton.
Unsequenced buckets have sequence zero. Global bundles contain exactly
`backups`, `config`, and `projects`. Project bundles require the four historical
base buckets and all buckets introduced by their source format; they may also
contain later known buckets already created by legacy initialization. The final
40 bytes are:

```text
"HASH" | algorithm:u16=1 | digest_len:u16=32 | sha256[32]
```

The digest covers every byte before `HASH`. It detects corruption but does not
authenticate the producer. Validation is capped at 16 GiB, 13 buckets, one
million records total and per bucket, 255-byte names, 1 MiB keys, and 256 MiB
values. Retained import data is capped at 256 MiB after accounting for each raw
key, raw value, and its 20-byte destination record envelope.
