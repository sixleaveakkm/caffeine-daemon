use std::sync::Arc;
use std::time::{Duration, Instant};

use caffeine_daemon::caffeinate::Noop;
use caffeine_daemon::holds::{Clock, End, Holds};

struct FixedClock(Instant);

impl Clock for FixedClock {
    fn now(&self) -> Instant {
        self.0
    }
}

fn holds() -> Holds {
    Holds::new(
        Duration::from_secs(300),
        Arc::new(Noop::new()),
        Arc::new(FixedClock(Instant::now())),
    )
}

#[test]
fn a_hold_shows_up_in_status() {
    let holds = holds();
    holds.hold("s-1".into(), "my-project".into()).unwrap();

    let status = holds.status();
    assert!(status.awake);
    assert!(status.recent.is_empty());
    assert_eq!(status.holds.len(), 1);
    assert_eq!(status.holds[0].id, "s-1");
    assert_eq!(status.holds[0].title, "my-project");
    assert_eq!(status.holds[0].expires_in, 300);
}

#[test]
fn releasing_the_last_hold_lets_the_mac_sleep() {
    let holds = holds();
    holds.hold("s-1".into(), "my-project".into()).unwrap();
    holds.release("s-1").unwrap();

    let status = holds.status();
    assert!(!status.awake);
    assert!(status.holds.is_empty());
    assert_eq!(status.recent.len(), 1);
    assert_eq!(status.recent[0].id, "s-1");
    assert_eq!(status.recent[0].end, End::Released);
}

#[test]
fn status_serializes_to_the_documented_shape() {
    let holds = holds();
    holds.hold("s-1".into(), "my-project".into()).unwrap();

    let json = serde_json::to_string(&holds.status()).unwrap();
    assert!(json.contains(r#""awake":true"#));
    assert!(json.contains(r#""expires_in":300"#));
    assert!(json.contains(r#""recent":[]"#));
}

/// Exercises the wire path the `status` and `stop` subcommands take, against a
/// real server.
#[test]
fn the_client_can_read_status_and_stop_a_hold() {
    use caffeine_daemon::server::{self, Api};
    use std::net::TcpListener;
    use std::sync::Arc;

    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let bind = format!("127.0.0.1:{port}");

    let holds = Arc::new(holds());
    holds.hold("s-1".into(), "my-project".into()).unwrap();

    let api = Api {
        holds: holds.clone(),
        api_key: Some("s3cret".into()),
    };
    let served = bind.clone();
    std::thread::spawn(move || {
        let _ = server::serve(api, &served);
    });

    let status = loop {
        match caffeine_daemon::client::fetch(&bind) {
            Ok(status) => break status,
            Err(_) => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    assert_eq!(status.holds.len(), 1);
    assert!(status.awake);

    let refused = caffeine_daemon::client::release(&bind, "s-1", Some("wrong")).unwrap_err();
    assert!(refused.to_string().contains("401"), "{refused}");
    assert!(caffeine_daemon::client::fetch(&bind).unwrap().awake);

    caffeine_daemon::client::release(&bind, "s-1", Some("s3cret")).unwrap();
    let status = caffeine_daemon::client::fetch(&bind).unwrap();
    assert!(!status.awake);
    assert!(status.holds.is_empty());
    assert_eq!(status.recent[0].end, End::Released);
}
