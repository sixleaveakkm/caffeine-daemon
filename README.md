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

Configuration lives at `~/.config/caffeine-daemon/config.toml`. Both keys are optional;
the values below are the defaults.

```toml
# How long a hold survives without being refreshed.
ttl  = "5m"

# Address to listen on. 0.0.0.0 so containers can reach it through
# host.docker.internal; use 127.0.0.1 to accept host-local callers only.
bind = "0.0.0.0:8787"
```

Run it in the foreground to check the setup:

```sh
caffeine-daemon
```

## Run as a background service

macOS uses `launchd` rather than `systemd`. Because `caffeinate` has to run inside your
logged-in GUI session, this is a LaunchAgent, not a LaunchDaemon.

Write `~/Library/LaunchAgents/local.caffeine-daemon.plist`:

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

Load, check, and unload it:

```sh
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/local.caffeine-daemon.plist
launchctl print gui/$(id -u)/local.caffeine-daemon
launchctl bootout gui/$(id -u)/local.caffeine-daemon
```

After editing the plist or replacing the binary, `bootout` then `bootstrap` again.

## HTTP API

### `POST /hook`

```json
{
  "id":    "5f1c9a3e-...",
  "title": "my-project",
  "event": "hold"
}
```

| Field   | Meaning                                                              |
| ------- | -------------------------------------------------------------------- |
| `id`    | Identifies the hold. Use the session id so repeat calls refresh it.  |
| `title` | Human-readable label, shown in `/status`.                            |
| `event` | `hold` starts or refreshes the hold; `release` ends it.              |

A `hold` expires by itself after `ttl` if nothing refreshes it, so a session that dies
without sending `release` cannot pin the Mac awake indefinitely.

Responds `204 No Content` on success, `400` on a malformed body.

### `GET /status`

```json
{
  "awake": true,
  "holds": [
    { "id": "5f1c9a3e-...", "title": "my-project", "expires_in": 287 }
  ]
}
```

`awake` reports whether `caffeinate` is currently running. `expires_in` is seconds.

## Use with Claude Code hooks

Add this to `~/.claude/settings.json`. `PreToolUse` refreshes the hold while Claude works;
`Stop` and `SessionEnd` release it.

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "jq -c '{id: .session_id, title: (.cwd | split(\"/\") | last), event: \"hold\"}' | curl -sf -m 2 -X POST -H 'Content-Type: application/json' --data-binary @- http://127.0.0.1:8787/hook >/dev/null || true"
          }
        ]
      }
    ],
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "jq -c '{id: .session_id, title: (.cwd | split(\"/\") | last), event: \"release\"}' | curl -sf -m 2 -X POST -H 'Content-Type: application/json' --data-binary @- http://127.0.0.1:8787/hook >/dev/null || true"
          }
        ]
      }
    ],
    "SessionEnd": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "jq -c '{id: .session_id, title: (.cwd | split(\"/\") | last), event: \"release\"}' | curl -sf -m 2 -X POST -H 'Content-Type: application/json' --data-binary @- http://127.0.0.1:8787/hook >/dev/null || true"
          }
        ]
      }
    ]
  }
}
```

The `-m 2` and `|| true` keep a stopped daemon from stalling or failing your hooks.

### From Docker

When Claude Code runs in a container, keep `bind = "0.0.0.0:8787"` and point the hooks at
the host instead of loopback:

```
http://host.docker.internal:8787/hook
```

On Docker Desktop and OrbStack that name resolves to the Mac. Verify from inside the
container with:

```sh
curl -sf http://host.docker.internal:8787/status
```

## Verify it works

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
