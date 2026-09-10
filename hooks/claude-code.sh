#!/bin/bash
# caffeine-daemon hook for Claude Code. Print a fresh copy with
# `caffeine-daemon print-script`.
#
#   claude-code.sh hold      keep the Mac awake for this session
#   claude-code.sh release   let it sleep again
#
# Env: CAFFEINE_HOST (host.docker.internal), CAFFEINE_PORT (the daemon's port),
#      CAFFEINE_API_KEY (the daemon's key), CAFFEINE_TITLE,
#      CAFFEINE_REFRESH (60 seconds).
set -u
EVENT="${1:-hold}"
HOST="${CAFFEINE_HOST:-host.docker.internal}"
PORT="${CAFFEINE_PORT:-8787}" # default replaced by `caffeine-daemon print-script`
DEFAULT_KEY='' # default replaced by `caffeine-daemon print-script`
KEY="${CAFFEINE_API_KEY:-$DEFAULT_KEY}"
REFRESH="${CAFFEINE_REFRESH:-60}"
HOOK_JSON=$(cat)

payload=$(
  EVENT="$EVENT" HOOK_JSON="$HOOK_JSON" REFRESH="$REFRESH" \
    CAFFEINE_TITLE="${CAFFEINE_TITLE:-}" python3 - <<'PY' 2>/dev/null
import json, os, sys, time

try:
    d = json.loads(os.environ.get("HOOK_JSON") or "{}")
except Exception:
    d = {}

event = os.environ.get("EVENT") or "hold"
session = d.get("session_id") or "default"
safe = "".join(c for c in session if c.isalnum() or c in "-_") or "default"
stamp = os.path.join(os.path.expanduser("~/.claude/cache/caffeine-daemon"), safe)

if event == "release":
    try:
        os.remove(stamp)
    except Exception:
        pass
else:
    # PreToolUse fires on every tool call, and one refresh per window keeps the
    # hold alive just as well. Exit 3 tells the caller there is nothing to send.
    try:
        refresh = float(os.environ.get("REFRESH") or 60)
    except ValueError:
        refresh = 60.0
    try:
        if time.time() - os.path.getmtime(stamp) < refresh:
            sys.exit(3)
    except Exception:
        pass
    try:
        os.makedirs(os.path.dirname(stamp), exist_ok=True)
        open(stamp, "w").close()
    except Exception:
        pass

def project_name(cwd):
    # every container bind-mounts its project onto the same /workspace,
    # so only the host-side mount source identifies the project
    try:
        with open("/proc/self/mountinfo") as f:
            for line in f:
                p = line.split()
                if len(p) > 4 and p[4] == cwd and p[3] != "/":
                    return os.path.basename(p[3].rstrip("/"))
    except Exception:
        pass
    return os.path.basename((cwd or "").rstrip("/"))

title = os.environ.get("CAFFEINE_TITLE") or project_name(d.get("cwd") or "") or "claude"
sys.stdout.write(json.dumps({"id": session, "title": title, "event": event}))
PY
)
rc=$?

# 3 = refreshed recently enough, nothing worth sending
if [ "$rc" -eq 3 ]; then
  exit 0
fi

# Without python3 the session id is unavailable, so fall back to one shared hold
# rather than leaving the Mac to sleep mid-run.
if [ -z "${payload:-}" ]; then
  payload="{\"id\":\"claude-code\",\"title\":\"claude\",\"event\":\"${EVENT}\"}"
fi

auth=()
if [ -n "$KEY" ]; then
  auth=(-H "api-key: $KEY")
fi

# -m keeps a stopped daemon from stalling the hook; || true keeps it from failing.
curl -sf -m 2 -X POST -H 'Content-Type: application/json' \
  ${auth[@]+"${auth[@]}"} \
  --data-raw "$payload" \
  "http://${HOST}:${PORT}/hook" >/dev/null 2>&1 || true

exit 0
