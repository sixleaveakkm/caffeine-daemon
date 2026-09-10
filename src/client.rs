use std::error::Error;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::holds::{End, Status};

const TIMEOUT: Duration = Duration::from_secs(2);

/// Asks a running daemon for its status.
pub fn fetch(bind: &str) -> Result<Status, Box<dyn Error>> {
    let (_, body) = send(bind, "GET", "/status", None, None)?;
    Ok(serde_json::from_str(&body)?)
}

/// Ends a hold by id, as `POST /hook` with a `release` event.
pub fn release(bind: &str, id: &str, api_key: Option<&str>) -> Result<(), Box<dyn Error>> {
    let body = serde_json::json!({ "id": id, "event": "release" }).to_string();
    send(bind, "POST", "/hook", Some(&body), api_key)?;
    Ok(())
}

/// Hand-rolled because the daemon only needs a server, and its own replies are
/// always Content-Length'd.
fn send(
    bind: &str,
    method: &str,
    path: &str,
    body: Option<&str>,
    api_key: Option<&str>,
) -> Result<(u16, String), Box<dyn Error>> {
    let addr = resolve(bind)?;
    let mut stream = TcpStream::connect_timeout(&addr, TIMEOUT)
        .map_err(|err| format!("cannot reach the daemon at {addr}: {err}"))?;
    stream.set_read_timeout(Some(TIMEOUT)).ok();
    stream.set_write_timeout(Some(TIMEOUT)).ok();

    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    if let Some(key) = api_key {
        request.push_str(&format!("api-key: {key}\r\n"));
    }
    if let Some(body) = body {
        request.push_str("Content-Type: application/json\r\n");
        request.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    request.push_str("\r\n");
    if let Some(body) = body {
        request.push_str(body);
    }
    stream.write_all(request.as_bytes())?;

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    let response = String::from_utf8_lossy(&raw);
    let (head, body) = response
        .split_once("\r\n\r\n")
        .ok_or("the daemon sent a malformed response")?;
    let line = head.lines().next().unwrap_or_default().trim();
    let code: u16 = line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .ok_or("the daemon sent a malformed response")?;
    if !(200..300).contains(&code) {
        let detail = body.trim();
        return Err(match detail.is_empty() {
            true => format!("daemon answered {code}"),
            false => format!("daemon answered {code}: {detail}"),
        }
        .into());
    }
    Ok((code, body.to_owned()))
}

/// Renders a status for the terminal, with ANSI colour when `color` is set.
pub fn render(status: &Status, color: bool) -> String {
    let paint = Paint(color);
    let mut out = match (status.awake, status.pid) {
        (true, Some(pid)) => format!(
            "{} {} {}\n",
            paint.dim("caffeinate:"),
            paint.green("running"),
            paint.dim(&format!("(pid {pid})"))
        ),
        (true, None) => format!("{} {}\n", paint.dim("caffeinate:"), paint.green("running")),
        (false, _) => format!(
            "{} {}\n",
            paint.dim("caffeinate:"),
            paint.dim("not running")
        ),
    };
    out.push_str(&line(format!(
        "{} {}",
        paint.dim("api-key:"),
        match status.api_key_required {
            true => paint.green("required"),
            false => paint.dim("not required"),
        }
    )));

    out.push_str(&format!("\n{}\n", paint.bold("holds")));
    out.push_str(&line(paint.dim(&row(&[
        ("TITLE", TITLE_W),
        ("EXPIRES IN", WHEN_W),
        ("ID", 0),
    ]))));
    if status.holds.is_empty() {
        out.push_str(&line(paint.dim("  (none)")));
    }
    for hold in &status.holds {
        out.push_str(&cells(
            &[
                (clip(&hold.title), TITLE_W, Color::Plain),
                (humanize(hold.expires_in), WHEN_W, Color::Cyan),
                (hold.id.clone(), 0, Color::Dim),
            ],
            &paint,
        ));
    }

    out.push_str(&format!("\n{}\n", paint.bold("ended")));
    out.push_str(&line(paint.dim(&row(&[
        ("TITLE", TITLE_W),
        ("STATE", STATE_W),
        ("WHEN", WHEN_W),
        ("ID", 0),
    ]))));
    if status.recent.is_empty() {
        out.push_str(&line(paint.dim("  (none)")));
    }
    for ended in &status.recent {
        let state = match ended.end {
            End::Released => Color::Green,
            End::Expired => Color::Yellow,
        };
        out.push_str(&cells(
            &[
                (clip(&ended.title), TITLE_W, Color::Plain),
                (ended.end.as_str().to_owned(), STATE_W, state),
                (
                    format!("{} ago", humanize(ended.ended_ago)),
                    WHEN_W,
                    Color::Dim,
                ),
                (ended.id.clone(), 0, Color::Dim),
            ],
            &paint,
        ));
    }

    out
}

/// Whether to colour stdout: the flag wins, then NO_COLOR, then whether stdout
/// is a terminal at all.
pub fn use_color(disabled: bool) -> bool {
    if disabled || std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    unsafe { libc::isatty(libc::STDOUT_FILENO) == 1 }
}

const TITLE_W: usize = 24;
const STATE_W: usize = 10;
const WHEN_W: usize = 12;

enum Color {
    Plain,
    Dim,
    Cyan,
    Green,
    Yellow,
}

struct Paint(bool);

impl Paint {
    fn wrap(&self, text: &str, code: &str) -> String {
        match self.0 {
            true => format!("\x1b[{code}m{text}\x1b[0m"),
            false => text.to_owned(),
        }
    }

    fn dim(&self, text: &str) -> String {
        self.wrap(text, "2")
    }

    fn bold(&self, text: &str) -> String {
        self.wrap(text, "1")
    }

    fn green(&self, text: &str) -> String {
        self.wrap(text, "32")
    }

    fn paint(&self, text: &str, color: &Color) -> String {
        match color {
            Color::Plain => text.to_owned(),
            Color::Dim => self.wrap(text, "2"),
            Color::Cyan => self.wrap(text, "36"),
            Color::Green => self.wrap(text, "32"),
            Color::Yellow => self.wrap(text, "33"),
        }
    }
}

fn line(text: String) -> String {
    text + "\n"
}

/// Widths are applied before colouring, so escape codes never count as columns.
fn row(cells: &[(&str, usize)]) -> String {
    let mut line = String::from("  ");
    for (text, width) in cells {
        line.push_str(&format!("{text:<width$}  "));
    }
    line.trim_end().to_owned()
}

fn cells(cells: &[(String, usize, Color)], paint: &Paint) -> String {
    let mut row = String::from("  ");
    for (text, width, color) in cells {
        row.push_str(&paint.paint(&format!("{text:<width$}"), color));
        row.push_str("  ");
    }
    line(row.trim_end().to_owned())
}

/// A daemon bound to a wildcard address is reached over loopback.
fn resolve(bind: &str) -> Result<SocketAddr, Box<dyn Error>> {
    let (host, port) = bind
        .rsplit_once(':')
        .ok_or_else(|| format!("bind `{bind}` has no port"))?;
    let host = match host.trim_matches(['[', ']']) {
        "0.0.0.0" | "::" | "" => "127.0.0.1",
        other => other,
    };
    format!("{host}:{port}")
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| format!("bad address `{bind}`").into())
}

fn humanize(seconds: u64) -> String {
    humantime::format_duration(Duration::from_secs(seconds)).to_string()
}

fn clip(title: &str) -> String {
    const WIDTH: usize = 24;
    let mut chars: Vec<char> = title.chars().collect();
    if chars.len() <= WIDTH {
        return title.to_owned();
    }
    chars.truncate(WIDTH - 1);
    chars.into_iter().collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::holds::{Ended, Held};

    fn busy() -> Status {
        Status {
            awake: true,
            pid: Some(4242),
            api_key_required: true,
            holds: vec![Held {
                id: "s-1".into(),
                title: "my-project".into(),
                expires_in: 287,
            }],
            recent: vec![Ended {
                id: "s-0".into(),
                title: "old-project".into(),
                end: End::Expired,
                ended_ago: 60,
            }],
        }
    }

    #[test]
    fn wildcard_binds_are_reached_over_loopback() {
        assert_eq!(
            resolve("0.0.0.0:8787").unwrap().to_string(),
            "127.0.0.1:8787"
        );
        assert_eq!(
            resolve("127.0.0.1:8787").unwrap().to_string(),
            "127.0.0.1:8787"
        );
        assert!(resolve("0.0.0.0").is_err());
    }

    #[test]
    fn an_idle_daemon_renders_no_holds() {
        let rendered = render(
            &Status {
                awake: false,
                pid: None,
                api_key_required: false,
                holds: vec![],
                recent: vec![],
            },
            false,
        );
        assert!(rendered.starts_with("caffeinate: not running\napi-key: not required\n"));
        assert_eq!(rendered.matches("(none)").count(), 2);
    }

    #[test]
    fn a_busy_daemon_renders_pid_holds_and_history() {
        let rendered = render(&busy(), false);
        assert!(rendered.contains("running (pid 4242)"));
        assert!(rendered.contains("my-project"));
        assert!(rendered.contains("4m 47s"));
        assert!(rendered.contains("expired"));
        assert!(rendered.contains("1m ago"));
    }

    #[test]
    fn every_column_is_named_and_every_row_is_its_own_line() {
        let rendered = render(&busy(), false);
        for header in ["TITLE", "EXPIRES IN", "STATE", "WHEN", "ID"] {
            assert!(rendered.contains(header), "missing {header}");
        }
        assert_eq!(
            rendered.lines().collect::<Vec<_>>(),
            vec![
                "caffeinate: running (pid 4242)",
                "api-key: required",
                "",
                "holds",
                "  TITLE                     EXPIRES IN    ID",
                "  my-project                4m 47s        s-1",
                "",
                "ended",
                "  TITLE                     STATE       WHEN          ID",
                "  old-project               expired     1m ago        s-0",
            ]
        );
    }

    #[test]
    fn colour_is_opt_in_and_does_not_shift_the_columns() {
        let plain = render(&busy(), false);
        assert!(!plain.contains('\x1b'));

        let coloured = render(&busy(), true);
        assert!(coloured.contains("\x1b[2m"));
        let strip = |line: &str| {
            let mut out = String::new();
            let mut rest = line;
            while let Some(start) = rest.find('\x1b') {
                out.push_str(&rest[..start]);
                rest = &rest[rest[start..].find('m').unwrap() + start + 1..];
            }
            out + rest
        };
        for (plain, coloured) in plain.lines().zip(coloured.lines()) {
            assert_eq!(plain, strip(coloured));
        }
    }
}
