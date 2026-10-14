# Compatibility Contract

The binding technical spec for PgBackRust. Every artifact named here is a compatibility
surface. Byte parity with the reference implementation is the requirement unless the
user approved a deviation (section 10, currently empty).

Reference implementation: `/root/pgbackrest` (authoritative. read-only).
The C source and its tests are the spec. This document is the map to the spec.

## 1. Repository layout (on-disk structure)

The exact tree pgBackRest creates in repo-path. Directory names. file names. indices.

Sources of truth:
- src/storage (driver implementations)
- src/command/stanza (create. delete. upgrade)
- src/info (infoBackup. infoArchive. infoPg)

Key artifacts:
- `<repo>/backup/<stanza>/backup.info` — backup history. versioned
- `<repo>/backup/<stanza>/archive.info` — archive + cipher state
- `<repo>/backup/<stanza>/<label>/backup.manifest` — full backup metadata
- `<repo>/archive/<stanza>/<pg-version>/<db-id>/<wal-segment>` — WAL archive layout
- `<repo>/` marker files and locks (pgBackRest writes/removes `.lock` files)

Rules: version headers and format-version fields must be honored exactly. Unknown
fields in an info file must not crash (upstream tolerates forward/backward compat
according to its own logic. mirror that logic).

## 2. The manifest

Sources of truth:
- src/info/manifest/serialize.c.inc (serialization. versioned)
- src/info/manifest/build.c.inc (construction)
- src/info/manifest/store.c.inc (store/load)
- src/info/manifest/link.c.inc (hardlink handling)
- test/src/module/info/manifestTest.c

Requirements:
- Byte-identical manifest serialization for equivalent inputs
- Every format version ever emitted must parse. PgBackRest reads its own ancient
  manifests. So must we. This is a hard deliverable. Version upgrade logic included
- Field ordering. key naming. escaping rules. indentation. terminator bytes. All exact

## 3. Info files (backup.info and archive.info)

Sources of truth: src/info/infoBackup.c. src/info/infoArchive.c. src/info/info.c.

- Versioned history format (backrest-format-version header)
- Backup history records. WAL archive records. db-id maps. cipher-subkey values
- Reads must handle all versions. Writes must emit the current-format version

## 4. WAL segment naming and timelines

Sources of truth: src/postgres/interface (crc32. page). src/command/archive.

- WAL 16MB segment naming (000000010000000000000001 style)
- Timeline fallback dirs (pgBackRest scans timeline dirs per its documented rules)
- archive-get must resolve segments across timelines exactly like upstream
- Partial/zeroed WAL handling and patrol (paranoid) options

## 5. Pack format (protocol wire format)

Sources of truth: src/common/type/pack.c. src/protocol. test/fuzz/pack.c.

- The custom binary serialization used on the protocol connection
- Byte layout for all types: bool. int. uint. string. blob. list. kvl (key-value list)
- PgBackRust implements pack natively. It must interop with upstream peer processes
  (mixed-version operation: new binary talking to old binary over SSH/TLS)
- Fuzz harness ported from upstream test/fuzz must pass on our reader

## 6. Encryption format

Sources of truth: src/common/format/cipherBlockFormat.c. src/common/crypto.

- cipherBlock format: AES-256-CBC with HMAC. subkey derivation from a passphrase.
  cipher-subkey in backup.info/archive.info
- Byte-exact key derivation. salt handling. encryption of manifest and info files
- DO NOT modernize. Do not add AEAD. The format is the contract. (Upstream recently
  hardened RNG failure handling in 2.59.3. Port that behavior.)

## 7. Compression streams

Sources of truth: src/common/compress (gz. bz2. lz4. zst). src/common/io/filter.

- Filter chain model: io read/write filters compose compression + crypto
- Compression levels and defaults. file extension rules (.gz. .bz2. .lz4. .zst)
- Compressed artifacts must decompress with upstream libs and vice versa

## 8. Command behavior contract

Sources of truth: src/command/*. build/config.yaml (command roles. locks. log files).

Per command: CLI surface. option defaults. exit codes. log lines. lock acquisition
order and types (backup lock vs archive lock). async vs local vs remote roles.
stanza-create. stanza-delete. stanza-upgrade. backup (full/diff/incr + block incr +
resume + page checksums + backup-standby). restore (delta. remap. tablespaces.
timelines. block delta). archive-push/get (async queue). check. expire. info.
verify. annotate. manifest. repo-get/ls/rm. server. start/stop. help.

Exit codes and error codes come from build/error.yaml (generated tables). Mirror them.

## 9. Performance baselines

Recorded in .upstream/perf/ after the first ported benchmark run. Until then the
targets are upstream measurements on the same hardware. Tolerances: no change greater
than 5% on any baseline metric. Metrics: backup MB/s. restore MB/s. archive-push
latency. peak RSS during manifest-write on large backups. startup time.

## 10. Approved deviations

None. This list stays empty unless the user explicitly approves a deviation. Approved
deviations are documented with: date. reason. exact scope. affected artifact.