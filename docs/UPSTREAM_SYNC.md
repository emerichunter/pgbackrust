# Upstream Sync Process

PgBackRust tracks pgBackRest forever. Every upstream change is detected. classified.
and acted on without ever breaking compatibility.

## Mechanism

- scripts/upstream-sync.sh runs daily via cron (08:00 UTC. deliver to origin chat)
- It fetches the upstream repo at /root/pgbackrest (read-only reference)
- It compares the fetched tip against .upstream/state.json (last-processed commit)
- For each new upstream commit it maps touched C files to PgBackRust crates via
  .upstream/modules.json and writes a classification report to .upstream/reports/
- The report lands in the chat for user review. No auto-commit. No auto-merge.

## Classification

For every upstream commit the script classifies impact:

- `PORTED` — touches a crate already ported in PgBackRust. ACTION REQUIRED.
- `PENDING` — touches a crate not yet ported. Record in ledger only.
- `INFRA` — build. docs. CI. no runtime behavior. Record only.
- `SECURITY` — any security-relevant change (crypto. permissions. protocol).
  Always ACTION REQUIRED even if the crate is unported. Record and escalate.

## Action rules (agent or human executes)

PORTED crate + upstream change:

1. Port the change into the matching crate (translate intent. follow AGENTS.md)
2. Regenerate/refresh golden files if upstream changed test expectations
3. Run parity tests both directions (old artifact -> new code. new artifact -> upstream)
4. Run the affected command benchmarks. No regression beyond 5%
5. Update FEATURE_PARITY.md. Commit with the convention from AGENTS.md section 6
6. Update .upstream/state.json to the processed commit

PENDING crate + upstream change:

1. Append a note in FEATURE_PARITY.md under the crate row (upstream commit hash + summary)
2. Update .upstream/state.json. The port will absorb the change later

SECURITY anywhere:

1. Stop normal work. Assess immediately. Patch our crate if ported
2. If unported: note in ledger AND flag to the user with the upstream advisory
3. Nothing ships until the assessment is written down

## Rules

- The sync script never writes to /root/pgbackrest (reference stays pristine)
- state.json only moves forward. Never rewrite history to hide processed commits
- A missed day is fine. A missed SECURITY change is not. The script flags gaps
- Mixed-version interop is the enforcement: every ported crate must round-trip against
  the real upstream binary at the CURRENT upstream tip
- If upstream refactors a module we fully ported (e.g. storage rewrite). port the
  refactor at the same abstraction level. Rerun all parity. Do NOT freeze our version
  at an older upstream state. Drift is not acceptable

## State file format

.upstream/state.json:

```json
{
  "upstream_repo": "/root/pgbackrest",
  "last_processed": "4c65010e7d28ea68571732fb275d91266a97ad80",
  "last_fetched": "2026-10-14T00:00:00Z",
  "processed_count": 0
}
```

## Report format

.upstream/reports/YYYY-MM-DD.md. Summary counts + per-commit rows:

```
## 2026-10-14
- fetched: abc1234 (2 commits new)
- PORTED: 1 (commit def5678 -> crate-storage-posix)
- PENDING: 1 (commit 9ab1234 -> crate-storage-s3)
- SECURITY: 0
```

## Redundancy (DRP)

Primary: daily cron. Backup: manual run anytime (`bash scripts/upstream-sync.sh`).
Optional: GitHub webhook on pgbackrest/pgbackrest push events (requires a publicly
reachable webhook URL. ask the user before setting up).