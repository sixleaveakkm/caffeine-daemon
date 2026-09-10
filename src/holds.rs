use std::collections::{HashMap, VecDeque};
use std::io;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::caffeinate::Inhibitor;

/// How many ids `status` can look back over.
const HISTORY: usize = 32;

/// Time as a dependency, so the TTL can be tested without waiting for it.
pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// The `GET /status` payload, shared by the server and the `status` subcommand.
#[derive(Debug, Deserialize, Serialize)]
pub struct Status {
    pub awake: bool,
    pub pid: Option<u32>,
    /// Whether `POST /hook` demands an api-key. Set by the server, which is
    /// what owns the key; the holds themselves know nothing about it.
    #[serde(default)]
    pub api_key_required: bool,
    pub holds: Vec<Held>,
    pub recent: Vec<Ended>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Held {
    pub id: String,
    pub title: String,
    pub expires_in: u64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Ended {
    pub id: String,
    pub title: String,
    pub end: End,
    pub ended_ago: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum End {
    Released,
    Expired,
}

impl End {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Released => "released",
            Self::Expired => "expired",
        }
    }
}

/// Tracks who is holding and keeps the inhibitor in step with them.
pub struct Holds {
    state: Mutex<State>,
    inhibitor: Arc<dyn Inhibitor>,
    clock: Arc<dyn Clock>,
}

impl Holds {
    pub fn new(ttl: Duration, inhibitor: Arc<dyn Inhibitor>, clock: Arc<dyn Clock>) -> Self {
        Self {
            state: Mutex::new(State {
                live: HashMap::new(),
                past: VecDeque::new(),
                ttl,
            }),
            inhibitor,
            clock,
        }
    }

    /// Starts a hold, or refreshes it if the id is already known.
    pub fn hold(&self, id: String, title: String) -> io::Result<()> {
        let now = self.clock.now();
        // The lock spans the inhibitor call so concurrent hooks cannot engage
        // and release out of order.
        let mut state = self.state();
        let was_active = state.is_active();
        state.expire(now);
        let expires_at = now + state.ttl;
        match state.live.get_mut(&id) {
            Some(hold) => {
                hold.title = title;
                hold.expires_at = expires_at;
            }
            None => {
                state.live.insert(id, Hold { title, expires_at });
            }
        }
        self.sync(was_active, state.is_active())
    }

    pub fn release(&self, id: &str) -> io::Result<()> {
        let now = self.clock.now();
        let mut state = self.state();
        let was_active = state.is_active();
        if let Some(hold) = state.live.remove(id) {
            state.remember(id.to_owned(), hold, End::Released, now);
        }
        state.expire(now);
        self.sync(was_active, state.is_active())
    }

    /// Drops holds nobody refreshed in time, so a dead session cannot pin the
    /// Mac awake forever.
    pub fn sweep(&self) -> io::Result<()> {
        let now = self.clock.now();
        let mut state = self.state();
        let was_active = state.is_active();
        state.expire(now);
        self.sync(was_active, state.is_active())
    }

    pub fn status(&self) -> Status {
        let now = self.clock.now();
        let state = self.state();

        let mut holds: Vec<Held> = state
            .live
            .iter()
            .filter(|(_, hold)| hold.expires_at > now)
            .map(|(id, hold)| Held {
                id: id.clone(),
                title: hold.title.clone(),
                expires_in: (hold.expires_at - now).as_secs(),
            })
            .collect();
        holds.sort_by(|a, b| a.expires_in.cmp(&b.expires_in).then(a.id.cmp(&b.id)));

        Status {
            awake: self.inhibitor.engaged(),
            pid: self.inhibitor.pid(),
            api_key_required: false,
            holds,
            recent: state
                .past
                .iter()
                .map(|past| Ended {
                    id: past.id.clone(),
                    title: past.title.clone(),
                    end: past.end,
                    ended_ago: now.saturating_duration_since(past.at).as_secs(),
                })
                .collect(),
        }
    }

    fn sync(&self, was_active: bool, is_active: bool) -> io::Result<()> {
        match (was_active, is_active) {
            (false, true) => self.inhibitor.engage(),
            (true, false) => self.inhibitor.release(),
            _ => Ok(()),
        }
    }

    /// A panic while holding the lock leaves the state readable, and losing the
    /// daemon over it would strand a live `caffeinate`.
    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

struct Hold {
    title: String,
    expires_at: Instant,
}

struct Past {
    id: String,
    title: String,
    end: End,
    at: Instant,
}

struct State {
    live: HashMap<String, Hold>,
    past: VecDeque<Past>,
    ttl: Duration,
}

impl State {
    fn is_active(&self) -> bool {
        !self.live.is_empty()
    }

    fn expire(&mut self, now: Instant) {
        let expired: Vec<String> = self
            .live
            .iter()
            .filter(|(_, hold)| hold.expires_at <= now)
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            if let Some(hold) = self.live.remove(&id) {
                self.remember(id, hold, End::Expired, now);
            }
        }
    }

    /// Only an id's latest ending is kept: a session holds and releases over and
    /// over, and the earlier rounds say nothing the last one does not.
    fn remember(&mut self, id: String, hold: Hold, end: End, now: Instant) {
        self.past.retain(|past| past.id != id);
        self.past.push_front(Past {
            id,
            title: hold.title,
            end,
            at: now,
        });
        self.past.truncate(HISTORY);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caffeinate::Noop;

    struct TestClock {
        base: Instant,
        offset: Mutex<Duration>,
    }

    impl TestClock {
        fn new() -> Self {
            Self {
                base: Instant::now(),
                offset: Mutex::new(Duration::ZERO),
            }
        }

        fn advance(&self, by: Duration) {
            *self.offset.lock().unwrap() += by;
        }
    }

    impl Clock for TestClock {
        fn now(&self) -> Instant {
            self.base + *self.offset.lock().unwrap()
        }
    }

    fn fixture() -> (Holds, Arc<TestClock>) {
        let clock = Arc::new(TestClock::new());
        let holds = Holds::new(
            Duration::from_secs(300),
            Arc::new(Noop::new()),
            clock.clone(),
        );
        (holds, clock)
    }

    #[test]
    fn overlapping_holds_keep_it_awake_until_the_last_release() {
        let (holds, _clock) = fixture();
        holds.hold("a".into(), "a".into()).unwrap();
        holds.hold("b".into(), "b".into()).unwrap();
        assert!(holds.status().awake);

        holds.release("a").unwrap();
        assert!(holds.status().awake);
        holds.release("b").unwrap();
        assert!(!holds.status().awake);
    }

    #[test]
    fn refreshing_extends_the_same_hold() {
        let (holds, clock) = fixture();
        holds.hold("a".into(), "first".into()).unwrap();
        clock.advance(Duration::from_secs(200));
        holds.hold("a".into(), "second".into()).unwrap();
        clock.advance(Duration::from_secs(200));

        let status = holds.status();
        assert_eq!(status.holds.len(), 1);
        assert_eq!(status.holds[0].title, "second");
        assert_eq!(status.holds[0].expires_in, 100);
    }

    #[test]
    fn sweep_releases_an_abandoned_hold() {
        let (holds, clock) = fixture();
        holds.hold("a".into(), "a".into()).unwrap();
        clock.advance(Duration::from_secs(301));
        holds.sweep().unwrap();

        let status = holds.status();
        assert!(!status.awake);
        assert!(status.holds.is_empty());
        assert_eq!(status.recent[0].end, End::Expired);
    }

    #[test]
    fn history_lists_ended_holds_newest_first_and_is_capped() {
        let (holds, clock) = fixture();
        holds.hold("a".into(), "a".into()).unwrap();
        holds.release("a").unwrap();
        clock.advance(Duration::from_secs(60));
        holds.hold("b".into(), "b".into()).unwrap();
        holds.release("b").unwrap();

        let recent = holds.status().recent;
        assert_eq!(recent[0].id, "b");
        assert_eq!(recent[0].ended_ago, 0);
        assert_eq!(recent[1].id, "a");
        assert_eq!(recent[1].ended_ago, 60);
        assert_eq!(recent[1].end, End::Released);

        for n in 0..HISTORY {
            let id = format!("h-{n}");
            holds.hold(id.clone(), "x".into()).unwrap();
            holds.release(&id).unwrap();
        }
        assert_eq!(holds.status().recent.len(), HISTORY);
    }

    #[test]
    fn one_id_that_ends_repeatedly_is_a_single_row() {
        let (holds, clock) = fixture();
        for _ in 0..3 {
            holds.hold("s-1".into(), "my-project".into()).unwrap();
            holds.release("s-1").unwrap();
            clock.advance(Duration::from_secs(10));
        }
        holds.hold("s-2".into(), "other".into()).unwrap();
        clock.advance(Duration::from_secs(1000));
        holds.sweep().unwrap();

        let recent = holds.status().recent;
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].id, "s-2");
        assert_eq!(recent[0].end, End::Expired);
        assert_eq!(recent[1].id, "s-1");
        assert_eq!(recent[1].end, End::Released);
        // the newest of that id's endings (t=20), not the oldest (t=0)
        assert_eq!(recent[1].ended_ago, 1010);
    }
}
