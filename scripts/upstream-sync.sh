#!/usr/bin/env bash
#
# PgBackRust upstream sync. Fetch pgBackRest. classify new commits. write report.
# Never writes to the upstream repo. Never commits. Never pushes.
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
UPSTREAM="${UPSTREAM_REPO:-/root/pgbackrest}"
STATE="$ROOT/.upstream/state.json"
MODULES="$ROOT/.upstream/modules.json"
REPORTS="$ROOT/.upstream/reports"
STAMP="$(date -u +%Y-%m-%d)"
REPORT="$REPORTS/$STAMP.md"

# ---------------------------------------------------------------- fetch upstream
if [ ! -d "$UPSTREAM/.git" ]; then
  echo "ERROR: upstream repo not found at $UPSTREAM" >&2
  exit 1
fi

git -C "$UPSTREAM" fetch origin --quiet 2>/dev/null || git -C "$UPSTREAM" fetch --quiet
UPSTREAM_BRANCH="$(git -C "$UPSTREAM" symbolic-ref refs/remotes/origin/HEAD 2>/dev/null | sed 's|refs/remotes/||' || echo origin/master)"
TIP="$(git -C "$UPSTREAM" rev-parse "$UPSTREAM_BRANCH")"
TIP_DATE="$(git -C "$UPSTREAM" log -1 --format=%ci "$TIP")"

# ------------------------------------------------------------------ load state (python3: state.json + modules.json)
eval "$(python3 - "$STATE" "$TIP" "$STAMP" <<'PYEOF'
import json, sys, datetime
state_path, tip, stamp = sys.argv[1], sys.argv[2], sys.argv[3]
try:
    with open(state_path) as f: state = json.load(f)
except Exception:
    state = {"last_processed": None}
last = state.get("last_processed")
print(f'LAST_PROCESSED={last or ""}')
print(f'PREV_COUNT={state.get("processed_count", 0)}')
PYEOF
)"

# ------------------------------------------------------------------ no new commits?
if [ "$LAST_PROCESSED" = "$TIP" ]; then
  echo "Up to date at $TIP (no new upstream commits)"
  python3 - "$STATE" "$TIP" "$STAMP" "$PREV_COUNT" <<'PYEOF'
import json, sys
state_path, tip, stamp, count = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
state = json.load(open(state_path))
state["last_fetched"] = stamp
json.dump(state, open(state_path, "w"), indent=2)
PYEOF
  exit 0
fi

# ------------------------------------------------------------------ classify commits
python3 - "$UPSTREAM" "$LAST_PROCESSED" "$TIP" "$MODULES" "$REPORT" <<'PYEOF'
import json, subprocess, sys, os

upstream, last, tip, modules_path, report_path = sys.argv[1:6]
mod = json.load(open(modules_path))
mapping = mod["mapping"]
security_pat = mod.get("security_patterns", [])
ignored_pat = mod.get("ignored_paths", [])
import re
ignored_re = [re.compile(p) for p in ignored_pat]

def classify(files):
    best = ("INFRA", None)
    for f in files:
        if any(r.search(f) for r in ignored_re):
            continue
        sec = any(p in f.lower() for p in security_pat)
        crates = [m["crate"] for m in mapping if f.startswith(m["pattern"])]
        if not crates and not sec:
            continue
        if sec and best[0] != "SECURITY":
            best = ("SECURITY", crates[0] if crates else None)
        elif best[0] in ("INFRA",) and crates:
            status = "PORTED" if any(m["crate"] == crates[0] and m["status"] == "ported" for m in mapping) else "PENDING"
            best = (status, crates[0])
    return best

if not last:
    print("No baseline. Seeding state to tip. Run again tomorrow for a delta report.")
    base = tip + "^"  # seed: show nothing
else:
    base = last

out = subprocess.run(
    ["git", "-C", upstream, "log", "--name-only", "--format=%H%x09%s", f"{base}..{tip}"],
    capture_output=True, text=True, check=True).stdout

rows = []
cur = None
for line in out.splitlines():
    if not line.strip():
        continue
    if "\t" in line:
        parts = line.split("\t", 1)
        if len(parts) == 2 and len(parts[0]) == 40:
            cur = {"hash": parts[0][:12], "subject": parts[1], "files": [], "kind": "INFRA"}
            rows.append(cur)
            continue
    if cur is not None:
        cur["files"].append(line.strip())

counts = {"PORTED": 0, "PENDING": 0, "SECURITY": 0, "INFRA": 0}
for r in rows:
    kind, crate = classify(r["files"])
    r["kind"], r["crate"] = kind, crate
    counts[kind] = counts.get(kind, 0) + 1

os.makedirs(os.path.dirname(report_path), exist_ok=True)
with open(report_path + ".tmp", "w") as f:
    f.write(f"## {os.path.basename(report_path)[:-3]}\n\n")
    f.write(f"- upstream tip: {tip[:12]} ({subprocess.run(['git','-C',upstream,'log','-1','--format=%ci',tip],capture_output=True,text=True).stdout.strip()})\n")
    f.write(f"- new commits since {last or 'seed'}: {len(rows)}\n")
    f.write(f"- PORTED: {counts.get('PORTED',0)} | PENDING: {counts.get('PENDING',0)} | SECURITY: {counts.get('SECURITY',0)} | INFRA: {counts.get('INFRA',0)}\n\n")
    if not rows:
        f.write("No commits classified (baseline seed).\n")
    for r in rows:
        f.write(f"- [{r['kind']}] {r['hash']} {r['subject']}\n")
        if r["crate"]:
            f.write(f"  - crate: {r['crate']}\n")
        if r["kind"] == "PORTED":
            f.write("  - ACTION: port. parity both directions. benchmark. commit.\n")
        if r["kind"] == "SECURITY":
            f.write("  - ACTION: assess immediately. escalate to user.\n")
os.replace(report_path + ".tmp", report_path)
print(f"report: {report_path}")
print(f"counts: {counts}")
PYEOF

# ------------------------------------------------------------------ update state
python3 - "$STATE" "$TIP" "$STAMP" "$PREV_COUNT" "$(python3 - "$REPORT" <<'PYEOF'
import json, sys
# count only classified commit rows ("- [KIND] ...")
rows = sum(1 for line in open(sys.argv[1]) if line.startswith("- ["))
print(rows)
PYEOF
)" <<'PYEOF'
import json, sys
state_path, tip, stamp, prev, added = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4]), int(sys.argv[5])
try:
    state = json.load(open(state_path))
except Exception:
    state = {}
state["last_processed"] = tip
state["last_fetched"] = stamp
state["processed_count"] = prev + added
json.dump(state, open(state_path, "w"), indent=2)
print(f"state updated: last_processed={tip[:12]} total={state['processed_count']}")
PYEOF