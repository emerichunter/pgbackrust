# PgBackRust

Rust rewrite of pgBackRest. Full byte-level compatibility. One-for-one feature parity.

- **Strategy**: Option B. Reads every artifact pgBackRest ever wrote. Writes artifacts
  pgBackRest reads. Both tools share one repository safely.
- **Reference**: /root/pgbackrest (read-only mirror of upstream)

Start here: [AGENTS.md](AGENTS.md) (mandatory reading. every session)
- [Feature parity ledger](docs/FEATURE_PARITY.md)
- [Compatibility contract](docs/COMPATIBILITY_CONTRACT.md)
- [Upstream sync](docs/UPSTREAM_SYNC.md)

Status: scaffolding + guidelines. No Rust code yet.

## Layout

- AGENTS.md — operating manual. iron rules. definition of done
- docs/ — ledger. contract. sync process
- scripts/upstream-sync.sh — daily upstream mirror + classification
- .upstream/ — sync state and reports
- crates/ — Rust workspace (empty until phase 0)

## Sync

Daily 08:00 UTC: fetch upstream. classify commits. report to origin chat. See
docs/UPSTREAM_SYNC.md.