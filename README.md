# caffeine-daemon

A small HTTP service for macOS that keeps your Mac awake while something is working.

It holds a `caffeinate -dis` process for as long as at least one caller has an active
hold. Holds are tracked by id, so several sessions can ask for wakefulness at once and
the Mac only sleeps again once the last one is gone.

It exists mainly for [Claude Code](https://claude.com/claude-code) hooks: Claude Code can
be running in a container, but only the host can stop the host from sleeping. The daemon
runs on the Mac, the hook posts to it over HTTP.

## Requirements

- macOS (the daemon shells out to `caffeinate`)
- Rust toolchain to build (`cargo`)
- `jq` and `curl` if you use the Claude Code hook examples below

## Install

```sh
git clone https://github.com/sixleaveakkm/caffeine-daemon
cd caffeine-daemon
cargo build --release
install -m 755 target/release/caffeine-daemon /usr/local/bin/caffeine-daemon
```

## Configure

Configuration lives at `~/.config/caffeine-daemon/config.toml`. Every key is optional;
the values below are the defaults.

```toml
# How long a hold survives without being refreshed.
ttl  = "5m"

# Address to listen on. 0.0.0.0 so containers can reach it through
# host.docker.internal; use 127.0.0.1 to accept host-local callers only.
bind = "0.0.0.0:8787"

# Shared secret. Unset, the API is open to anyone who can reach it.
# api-key = "a-long-random-string"
```

When `api-key` is set, `POST /hook` must present it, either as an `api-key` header or as
an `api-key` field in the JSON body. Anything else is answered `401 Unauthorized`.
`GET /status` is never checked.

Run it in the foreground to check the setup:

```sh
caffeine-daemon
```

## Commands

```sh
caffeine-daemon                      # start the server
caffeine-daemon status               # ask a running daemon what it is doing
caffeine-daemon stop <id>            # end one hold now, without waiting for its ttl
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
  program: /usr/local/bin/caffeine-daemon

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
pass `--force`. Either way, `bootout` and `bootstrap` again to pick the change up.

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
        <string>/usr/local/bin/caffeine-daemon</string>
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

When Claude Code runs in a container, keep `bind = "0.0.0.0:8787"`. The script already
defaults to `host.docker.internal`, which resolves to the Mac on Docker Desktop and
OrbStack, and it names the hold after the host-side project directory rather than the
container's mount point. Verify from inside the container with:

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
