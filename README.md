# caffeine-daemon

A small HTTP service for macOS that keeps your Mac awake while something is working.

It holds a `caffeinate -dis` process while at least one caller has an active hold. Holds
are tracked by id, so several callers can overlap and the Mac sleeps only once the last is
gone; each carries a ttl, so a caller that dies cannot pin it awake for good.

`caffeinate` only works from inside its own logged-in session. Anything that can reach
this daemon over HTTP can keep the Mac awake instead — a container, another machine, a
script. That is what it was built for: [Claude Code](https://claude.com/claude-code) in a
container, with a [ready-made hook script](#use-with-claude-code-hooks).

## Requirements

The daemon runs on the Mac and needs:

- macOS (it shells out to `caffeinate`)
- a Rust toolchain to build it (`cargo`)

The hook runs wherever Claude Code does — inside the container when Claude Code is
containerised, on the Mac when it is not — and needs `python3` and `curl` there. Most
container images ship both; on macOS `python3` comes with the Xcode Command Line Tools
(`xcode-select --install`).

`python3` turns Claude Code's hook JSON into the request body. Without it the hook still
keeps the Mac awake, but every session shares a single hold called `claude-code`: `status`
can no longer tell them apart, and the first session to stop releases it for all of them.

## Install

```sh
cargo install --git https://github.com/sixleaveakkm/caffeine-daemon
```

Or `cargo install --path .` from a clone. Either puts `caffeine-daemon` in `~/.cargo/bin`,
which `install-launchagent` then points the plist at.

## Configure

Configuration lives at `~/.config/caffeine-daemon/config.toml`. Every key is optional;
the values below are the defaults.

```toml
# How long a hold survives without being refreshed.
ttl  = "10m"

# Address to listen on. 0.0.0.0 so containers can reach it through
# host.docker.internal; use 127.0.0.1 to accept host-local callers only.
bind = "0.0.0.0:8787"

# Shared secret for POST /hook. Unset, the API is open to anyone who can
# reach it; see "The api-key" below.
# api-key = "a-long-random-string"
```

Unknown keys are refused rather than ignored, so a typo — `api_key` for `api-key`, say —
stops the daemon at startup instead of silently doing nothing. The file is read once, so
edits need a restart.

Run it in the foreground to check the setup:

```sh
caffeine-daemon
```

## The api-key

`bind = "0.0.0.0:8787"` is what lets a container reach the daemon, and it is also what
lets everything else on the network reach it. Set `api-key` to close that off:

```toml
api-key = "a-long-random-string"   # openssl rand -hex 16
```

It guards `POST /hook` — the endpoint that changes something. A caller presents it either
way round:

```sh
curl -H "api-key: $KEY" -d '{"id":"x","title":"x","event":"hold"}' http://127.0.0.1:8787/hook
curl -d '{"id":"x","title":"x","event":"hold","api-key":"'"$KEY"'"}' http://127.0.0.1:8787/hook
```

Anything else is `401 Unauthorized`. An empty `api-key = ""` counts as unset, so it opens
the API rather than locking every caller out.

`GET /status` is never checked. It reports `api_key_required` instead, so a caller can
tell a daemon that wants a key from one that is simply broken, without holding a key.

The pieces that ship with the daemon pick the key up on their own:

- `caffeine-daemon stop <id>` reads it from the same config file.
- `caffeine-daemon print-script` bakes it into the hook script it prints, and
  `CAFFEINE_API_KEY` in the environment overrides that.

A hook with the wrong key is answered `401`, and — by design — still exits `0` and prints
nothing, so an unauthorised session looks exactly like a working one from the inside. The
symptom is the Mac sleeping and no hold in `status`. Check `caffeine-daemon status`: it
prints `api-key: required` when the daemon wants one, and lists what is actually held.

## Commands

```sh
caffeine-daemon                      # start the server
caffeine-daemon status               # ask a running daemon what it is doing
caffeine-daemon stop <id>            # end one hold now, without waiting for its ttl
caffeine-daemon print-script         # print the Claude Code hook script
caffeine-daemon install-launchagent  # write the launchd plist
caffeine-daemon --help               # usage
```

`stop` takes an id from `status` and releases that hold, printing
`released 5f1c9a3e-... (my-project)`. An id nobody is holding is an error, and the
`api-key` is sent from the config when one is set.

Only one daemon runs at a time, whatever `bind` says: it takes an exclusive lock on
`$TMPDIR/caffeine-daemon.lock`, and a second one exits with `another caffeine-daemon is
already running (pid N)`. The kernel releases the lock however the process dies, so a
crash never blocks the next start.

`status` queries the daemon over HTTP at the configured `bind` address and prints
whether `caffeinate` is running (with its pid), the live holds, and the holds that
have ended:

```
caffeinate: running (pid 51234)
api-key: required

holds
  TITLE                     EXPIRES IN    ID
  my-project                4m 47s        5f1c9a3e-...

ended
  TITLE                     STATE       WHEN          ID
  other-project             released    2m 8s ago     0c2b7d10-...
  stale-session             expired     11m 3s ago    b91f4a55-...
```

Output is coloured when stdout is a terminal. Pass `--no-color`, or set `NO_COLOR`, to
turn that off; piping to a file or another command drops it automatically.

## Run as a background service

macOS uses `launchd` rather than `systemd`. Because `caffeinate` has to run inside your
logged-in GUI session, this is a LaunchAgent, not a LaunchDaemon.

`install-launchagent` writes the plist for you, pointing it at wherever
`caffeine-daemon` sits on your `PATH`:

```sh
caffeine-daemon install-launchagent
```

```
wrote /Users/you/Library/LaunchAgents/local.caffeine-daemon.plist
  program: /Users/you/.cargo/bin/caffeine-daemon

next:
  launchctl bootout gui/501/local.caffeine-daemon 2>/dev/null
  launchctl bootstrap gui/501 /Users/you/Library/LaunchAgents/local.caffeine-daemon.plist
```

Loading it is left to you, because `launchctl` has to run from the GUI session that
`caffeinate` needs. Run the two printed commands: `bootout` first, since bootstrapping a
label that is already loaded fails with `Bootstrap failed: 5: Input/output error`, and so
does bootstrapping a plist with a syntax error. Check it took with:

```sh
launchctl print gui/$(id -u)/local.caffeine-daemon
```

Re-run `install-launchagent` after moving the binary. It rewrites the plist only when the
contents would change, and refuses to overwrite a plist you have edited by hand unless you
pass `--force`.

Restarting it depends on what changed. launchd re-reads the plist only when you bootstrap,
so a rewritten plist needs the longer form:

```sh
# config.toml edited, or the binary replaced where it stands
launchctl kickstart -k gui/$(id -u)/local.caffeine-daemon

# plist rewritten by install-launchagent
launchctl bootout gui/$(id -u)/local.caffeine-daemon
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/local.caffeine-daemon.plist
```

`kickstart -k` stops the running copy and starts it again from the plist already loaded.
Plain `launchctl stop` is not a restart here — with `KeepAlive` set, launchd starts it
straight back up, which is also why killing the process by pid brings it back.

Either way the restart drops every live hold, since holds live only in memory, and the
`caffeinate` it was holding exits with it. The Mac can sleep until the next caller posts —
for Claude Code, the next hook event.

The plist it writes:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>local.caffeine-daemon</string>
    <key>ProgramArguments</key>
    <array>
        <string>/Users/you/.cargo/bin/caffeine-daemon</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/tmp/caffeine-daemon.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/caffeine-daemon.err</string>
</dict>
</plist>
```

`KeepAlive` restarts the daemon whenever it exits, so make sure no copy is already running
before you bootstrap: the second one exits on the lock, launchd restarts it, and you get a
throttled crash loop instead of a service. `launchctl print` shows a climbing run count
when that happens.

## HTTP API

### `POST /hook`

```json
{
  "id":    "5f1c9a3e-...",
  "title": "my-project",
  "event": "hold"
}
```

| Field     | Meaning                                                              |
| --------- | -------------------------------------------------------------------- |
| `id`      | Identifies the hold. Use the session id so repeat calls refresh it.  |
| `title`   | Human-readable label, shown in `/status`.                            |
| `event`   | `hold` starts or refreshes the hold; `release` ends it.              |
| `api-key` | Only when one is configured; an `api-key` header works as well.      |

A hold expires by itself after `ttl` if nothing refreshes it, so a session that dies
without sending `release` cannot pin the Mac awake indefinitely. Expiry is checked once
per `ttl`, so the Mac can stay awake up to `2 x ttl` after the last refresh.

Responds `204 No Content` on success, `400` on a malformed body, and `401` when a
configured `api-key` is missing or wrong.

### `GET /status`

```json
{
  "awake": true,
  "pid": 51234,
  "api_key_required": true,
  "holds": [
    { "id": "5f1c9a3e-...", "title": "my-project", "expires_in": 287 }
  ],
  "recent": [
    { "id": "0c2b7d10-...", "title": "other-project", "end": "released", "ended_ago": 128 }
  ]
}
```

`awake` reports whether `caffeinate` is currently running and `pid` is its process id,
`null` when nothing is held. `api_key_required` says whether `POST /hook` demands a key,
so a caller can tell a missing key from a broken hook. `recent` lists the last 32 ids to end, newest first,
each with `end` of `released` or `expired`. Only an id's latest ending is kept, so a
session that holds and releases repeatedly stays one row. Both `expires_in` and
`ended_ago` are seconds.

## Use with Claude Code hooks

`caffeine-daemon print-script` writes a hook script that reads Claude Code's hook JSON on
stdin and posts to the daemon. It bakes in the port and `api-key` from your config, so
print it again after changing either. Save it once:

```sh
mkdir -p ~/.claude/hooks
caffeine-daemon print-script > ~/.claude/hooks/caffeine.sh
chmod +x ~/.claude/hooks/caffeine.sh
```

With an `api-key` configured the saved file holds your key in plaintext, so keep it out of
anything you share (`chmod 600` it if the machine has other users). Setting
`CAFFEINE_API_KEY` in the environment instead leaves the file secret-free.

Then in `~/.claude/settings.json`. Every event that means "this session is still alive"
sends `hold`; the two that end a session send `release`.

```json
{
  "hooks": {
    "SessionStart": [
      { "hooks": [{ "type": "command", "command": "~/.claude/hooks/caffeine.sh hold" }] }
    ],
    "UserPromptSubmit": [
      { "hooks": [{ "type": "command", "command": "~/.claude/hooks/caffeine.sh hold" }] }
    ],
    "PreToolUse": [
      { "hooks": [{ "type": "command", "command": "~/.claude/hooks/caffeine.sh hold" }] }
    ],
    "PostToolUse": [
      { "hooks": [{ "type": "command", "command": "~/.claude/hooks/caffeine.sh hold" }] }
    ],
    "Notification": [
      { "hooks": [{ "type": "command", "command": "~/.claude/hooks/caffeine.sh hold" }] }
    ],
    "Stop": [
      { "hooks": [{ "type": "command", "command": "~/.claude/hooks/caffeine.sh release" }] }
    ],
    "SessionEnd": [
      { "hooks": [{ "type": "command", "command": "~/.claude/hooks/caffeine.sh release" }] }
    ]
  }
}
```

Wiring up every one of these costs nothing: the script posts at most once per
`CAFFEINE_REFRESH` window whichever event fired, so more hooks buy a tighter grip on
"still alive" without more traffic. `PreToolUse` alone would be enough for a busy
session; `UserPromptSubmit` and `Notification` cover a session that is waiting on you,
and `PostToolUse` marks the end of a long tool call.

The hold id is the session id, so several sessions hold independently and repeat calls
refresh rather than pile up. A stopped daemon cannot stall or fail a hook: every call is
capped at 2 seconds and always exits `0`.

No hook fires *during* a single tool call, so a build that runs longer than `ttl` can let
the Mac sleep between `PreToolUse` and `PostToolUse`. Raise `ttl` past your longest tool
call if that is a problem.

| Variable            | Default                | Meaning                                    |
| ------------------- | ---------------------- | ------------------------------------------ |
| `CAFFEINE_HOST`     | `host.docker.internal` | Where the daemon is. `127.0.0.1` on the Mac |
| `CAFFEINE_PORT`     | the configured port    | Overrides the port printed into the script  |
| `CAFFEINE_API_KEY`  | the configured key     | Overrides the key printed into the script   |
| `CAFFEINE_TITLE`    | project directory      | Label shown in `status`                     |
| `CAFFEINE_REFRESH`  | `60`                   | Seconds between refreshes; keep under `ttl` |

### From Docker

When Claude Code runs in a container, keep `bind = "0.0.0.0:8787"`. The script runs inside
the container, so that is where `python3` and `curl` have to be. It already defaults to
`host.docker.internal`, which resolves to the Mac on Docker Desktop and OrbStack, and it
names the hold after the host-side project directory rather than the container's mount
point. Verify from inside the container with:

```sh
curl -sf http://host.docker.internal:8787/status
```

On the Mac itself, set `CAFFEINE_HOST=127.0.0.1`.

## Verify it works

Add `-H "api-key: $KEY"` to the `/hook` calls if you configured one.

```sh
curl -sf -X POST -H 'Content-Type: application/json' \
  -d '{"id":"test","title":"manual check","event":"hold"}' \
  http://127.0.0.1:8787/hook

curl -sf http://127.0.0.1:8787/status
pmset -g assertions | grep caffeinate

curl -sf -X POST -H 'Content-Type: application/json' \
  -d '{"id":"test","title":"manual check","event":"release"}' \
  http://127.0.0.1:8787/hook
```

## License

Apache-2.0. See [LICENSE](LICENSE).
