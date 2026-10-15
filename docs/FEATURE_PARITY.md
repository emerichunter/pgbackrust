# Feature Parity Ledger

One-for-one feature parity with pgBackRest. Every row must reach `ported` with all
Definition-of-Done items true (see AGENTS.md section 4). The ledger is the tracking
truth. Machine status lives in .upstream/modules.json. Keep both in sync.

Upstream reference: /root/pgbackrest.

## Commands

| Command | C source | Crate | Status |
|---|---|---|---|
| stanza-create | src/command/stanza/create.c | crate-stanza | not-started |
| stanza-delete | src/command/stanza/delete.c | crate-stanza | not-started |
| stanza-upgrade | src/command/stanza/upgrade.c | crate-stanza | not-started |
| backup | src/command/backup/ | crate-backup | not-started |
| restore | src/command/restore/ | crate-restore | not-started |
| archive-push | src/command/archive/push/ | crate-archive | not-started |
| archive-get | src/command/archive/get/ | crate-archive | not-started |
| expire | src/command/expire/expire.c | crate-expire | not-started |
| info | src/command/info/info.c | crate-info | not-started |
| verify | src/command/verify/ | crate-verify | not-started |
| check | src/command/check/check.c | crate-check | not-started |
| annotate | src/command/annotate/annotate.c | crate-annotate | not-started |
| manifest | src/command/manifest/manifest.c | crate-manifest | not-started |
| repo-get/ls/rm | src/command/repo/ | crate-repo | not-started |
| server (ping/standby) | src/command/server/ | crate-server | not-started |
| start/stop | src/command/control/ | crate-control | not-started |
| help | src/command/help/help.c | crate-help | not-started |
| local/remote roles | src/command/local. src/command/remote | crate-command-core | not-started |

## Features (horizontal)

| Feature | C source | Proof requirement | Status |
|---|---|---|---|
| Full/diff/incr backups | src/command/backup/incr.c.inc | restore parity vs upstream | not-started |
| Block incremental | src/command/backup/blockIncr.c. blockMap.c src/command/restore/blockDelta.c | byte-identical block maps. round-trip | not-started |
| Backup resume | src/command/backup/resume.c.inc | resume after kill. parity | not-started |
| Page checksums | src/command/backup/pageChecksum.c | validate/warn parity | not-started |
| Delta restore | src/command/restore/ | delta mode parity | not-started |
| Tablespaces | src/command/restore/tablespace | remap + links parity | not-started |
| Backup-standby | src/command/backup/db.c.inc | standby discovery parity | not-started |
| Retention policies | src/command/expire | same retention math | not-started |
| Encryption | src/common/format/cipherBlockFormat.c | decrypt upstream repos | not-started |
| Compression 4 codecs | src/common/compress | cross-decompress | not-started |
| Hard links | src/info/manifest/link.c.inc | link semantics parity | not-started |
| Custom protocol/TLS/SSH | src/protocol | mixed-version interop | not-started |
| Parallelism (threads) | src/common/fork.c (upstream) | same worker math | not-started |
| async archiving | src/command/archive/push + queue dirs | async queue parity | not-started |
| Point-in-time repo reads | src/command (repo-* read-at options) | versioned storage reads | not-started |
| Fuzz (json/ini/pack) | test/fuzz | fuzz harness passes | not-started |

## Storage drivers

| Driver | C source | Crate | Status |
|---|---|---|---|
| posix | src/storage/posix | crate-storage-posix | not-started |
| remote (protocol proxy) | src/storage/remote | crate-storage-remote | not-started |
| s3 | src/storage/s3 | crate-storage-s3 | not-started |
| gcs | src/storage/gcs | crate-storage-gcs | not-started |
| azure | src/storage/azure | crate-storage-azure | not-started |
| sftp | src/storage/sftp | crate-storage-sftp | not-started |
| cifs | src/storage/cifs | crate-storage-cifs | not-started |

## Core subsystems

| Subsystem | C source | Crate | Status |
|---|---|---|---|
| Config + codegen | build/config.yaml src/config | crate-config | not-started |
| Error codes | build/error.yaml src/common/error | crate-error | ported (round 1. 8 tests) |
| Pack codec | src/common/type/pack.c | crate-pack | not-started |
| Types (buffer. list. kvl. variant. json. ini) | src/common/type | crate-types | ported (round 1. buffer+hex only. 12 tests) |
| IO + filters | src/common/io | crate-io | not-started |
| Crypto (hash. cipher) | src/common/crypto | crate-crypto | not-started |
| Lock/wait/stat/time | src/common (lock.c etc) | crate-common | not-started |
| Postgres interface | src/postgres/interface | crate-postgres | not-started |
| Manifest | src/info/manifest | crate-manifest | not-started |
| Info files | src/info | crate-info | not-started |
| Logging + stack traces | src/common/log.c src/common/stackTrace.c | crate-log | ported (round 1. byte-exact format. 12 tests) |
| Protocol client/server | src/protocol | crate-protocol | not-started |
| Parallel jobs | src/protocol/parallel* | crate-protocol | not-started |

## Port order (recommended. adjust with user approval)

1. crate-error + crate-log + crate-types + pack (foundation)
2. crate-config (codegen from upstream yaml)
3. crate-storage-posix + crate-io + compression filters
4. crate-manifest + crate-info (read ALL versions first. write current)
5. stanza lifecycle + check (first real commands)
6. archive-push/get (async queue)
7. backup full/diff/incr (resume. page checksums)
8. restore (delta. tablespaces. timelines)
9. expire + info + annotate + manifest + repo-* + help + control
10. protocol + parallel (local threads first. then TLS/SSH remote)
11. cloud drivers: s3. gcs. azure. sftp. cifs
12. block incremental + verify
13. fuzz + soak + perf parity lock-in

Each step ends with: all DoD items true. golden files committed. parity vs upstream
binary. mixed-version round-trip. benchmark recorded.