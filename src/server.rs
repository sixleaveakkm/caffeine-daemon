use std::io::Read;
use std::sync::Arc;

use serde::Deserialize;
use tiny_http::{Header, Method, Request, Response, Server};

use crate::holds::{Holds, Status};

/// Bodies are a few hundred bytes of JSON; anything larger is not ours.
const MAX_BODY: u64 = 64 * 1024;

#[derive(Debug, Deserialize)]
struct Hook {
    id: String,
    #[serde(default)]
    title: String,
    event: Event,
    #[serde(rename = "api-key", default)]
    api_key: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Event {
    Hold,
    Release,
}

/// The daemon as the API sees it: the holds, and who is allowed to touch them.
pub struct Api {
    pub holds: Arc<Holds>,
    /// `None` leaves `/hook` open; `Some` rejects callers without that key.
    pub api_key: Option<String>,
}

impl Api {
    fn authorized(&self, presented: Option<&str>) -> bool {
        match &self.api_key {
            None => true,
            Some(expected) => presented == Some(expected.as_str()),
        }
    }
}

/// Serves the API until the process is stopped.
pub fn serve(api: Api, bind: &str) -> Result<(), Box<dyn std::error::Error>> {
    let server = Server::http(bind).map_err(|err| format!("cannot bind {bind}: {err}"))?;
    // Startup notices go to stdout so a launchd StandardOutPath holds the
    // record of a healthy daemon and StandardErrorPath stays empty until
    // something is actually wrong.
    println!("caffeine-daemon: listening on http://{bind}");
    if api.api_key.is_some() {
        println!("caffeine-daemon: /hook requires an api-key");
    }

    // Hooks are small, local and rare, so requests are handled right here
    // rather than on a pool: tiny_http already reads connections on its own.
    for request in server.incoming_requests() {
        handle(&api, request);
    }
    Ok(())
}

fn handle(api: &Api, mut request: Request) {
    let method = request.method().clone();
    let path = request
        .url()
        .split(['?', '#'])
        .next()
        .unwrap_or("/")
        .to_owned();

    let reply = match (&method, path.as_str()) {
        (Method::Post, "/hook") => hook(api, &mut request),
        (Method::Get, "/status") => status(api),
        (_, "/hook" | "/status") => Reply::text(405, "method not allowed"),
        _ => Reply::text(404, "not found"),
    };
    if let Err(err) = respond(request, reply) {
        eprintln!("caffeine-daemon: cannot answer request: {err}");
    }
}

fn hook(api: &Api, request: &mut Request) -> Reply {
    let presented = header(request, "api-key");
    let mut body = String::new();
    if request
        .as_reader()
        .take(MAX_BODY)
        .read_to_string(&mut body)
        .is_err()
    {
        return Reply::text(400, "cannot read body");
    }
    // The key is checked before the body is trusted, so an unauthorized caller
    // learns nothing about what a valid body looks like.
    let parsed = serde_json::from_str::<Hook>(&body);
    let in_body = parsed
        .as_ref()
        .ok()
        .and_then(|hook| hook.api_key.as_deref());
    if !(api.authorized(presented.as_deref()) || api.authorized(in_body)) {
        return Reply::text(401, "unauthorized");
    }

    let hook = match parsed {
        Ok(hook) => hook,
        Err(err) => return Reply::text(400, &format!("invalid body: {err}")),
    };
    if hook.id.trim().is_empty() {
        return Reply::text(400, "id must not be empty");
    }

    let outcome = match hook.event {
        Event::Hold => api.holds.hold(hook.id, hook.title),
        Event::Release => api.holds.release(&hook.id),
    };
    match outcome {
        Ok(()) => Reply {
            status: 204,
            body: String::new(),
            json: false,
        },
        Err(err) => {
            eprintln!("caffeine-daemon: caffeinate failed: {err}");
            Reply::text(500, "caffeinate failed")
        }
    }
}

fn status(api: &Api) -> Reply {
    let status = Status {
        api_key_required: api.api_key.is_some(),
        ..api.holds.status()
    };
    match serde_json::to_string(&status) {
        Ok(json) => Reply {
            status: 200,
            body: json,
            json: true,
        },
        Err(err) => {
            eprintln!("caffeine-daemon: cannot encode status: {err}");
            Reply::text(500, "cannot encode status")
        }
    }
}

fn header(request: &Request, name: &'static str) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|header| header.field.equiv(name))
        .map(|header| header.value.as_str().to_owned())
}

struct Reply {
    status: u16,
    body: String,
    json: bool,
}

impl Reply {
    fn text(status: u16, body: &str) -> Self {
        Self {
            status,
            body: body.to_owned(),
            json: false,
        }
    }
}

fn respond(request: Request, reply: Reply) -> std::io::Result<()> {
    if reply.status == 204 {
        return request.respond(Response::empty(204));
    }
    let content_type = if reply.json {
        "application/json"
    } else {
        "text/plain"
    };
    let header =
        Header::from_bytes("Content-Type", content_type).expect("content type header is valid");
    request.respond(
        Response::from_string(reply.body)
            .with_status_code(reply.status)
            .with_header(header),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api(key: Option<&str>) -> Api {
        use crate::caffeinate::Noop;
        use crate::holds::SystemClock;
        use std::time::Duration;

        Api {
            holds: Arc::new(Holds::new(
                Duration::from_secs(300),
                Arc::new(Noop::new()),
                Arc::new(SystemClock),
            )),
            api_key: key.map(str::to_owned),
        }
    }

    #[test]
    fn without_a_configured_key_anyone_is_allowed() {
        let api = api(None);
        assert!(api.authorized(None));
        assert!(api.authorized(Some("anything")));
    }

    #[test]
    fn with_a_configured_key_only_that_key_is_allowed() {
        let api = api(Some("s3cret"));
        assert!(api.authorized(Some("s3cret")));
        assert!(!api.authorized(Some("wrong")));
        assert!(!api.authorized(Some("")));
        assert!(!api.authorized(None));
    }
}
