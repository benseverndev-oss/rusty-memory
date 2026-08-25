//! Sessions, for the transport that does not have one.
//!
//! Streamable HTTP gives every request its own connection and its own
//! [`Server`](crate::Server), so what `initialize` settles is gone by the time
//! the first tool call arrives. The specification's answer is a session id:
//! minted by the server at `initialize`, returned in `Mcp-Session-Id`, echoed
//! by the client on everything after it. This is the table behind that header.
//!
//! What it holds is a [`Handshake`] -- the client's name and the revision it
//! agreed on -- because those are the two things lost, and losing each is its
//! own bug. See [`Handshake`] for what they cost.
//!
//! # The ids are unguessable, but they are not secrets
//!
//! An id is 128 bits out of SipHash under a key `RandomState` takes from the
//! operating system. That is a keyed pseudo-random function over OS entropy,
//! which is enough to make ids unguessable, and it is *not* a certified
//! CSPRNG: std makes no such promise about `RandomState`, and successive keys
//! on one thread are related rather than independent.
//!
//! It is the right trade here because of what an id is worth. Reaching this
//! server at all needs network reach and, off loopback, the bearer token --
//! both checked before any of this runs. So a guessed id buys an attacker who
//! can *already write* the ability to write under another client's name. That
//! is attribution forgery, not an access-control bypass, and the store's own
//! provenance was never a security boundary: any client may pass whatever
//! `session` argument it likes.
//!
//! The alternative was a dependency or a `cfg` fork -- `/dev/urandom` does not
//! exist on Windows, which is a platform this is measured on -- and this
//! workspace has turned that trade down five times now.

use std::collections::hash_map::RandomState;
use std::collections::HashMap;
use std::hash::BuildHasher;
use std::sync::Mutex;

use rm_engine::Timestamp;

use crate::Handshake;

/// The most sessions kept at once.
///
/// A bound rather than a tuning knob: without one, a client that handshakes
/// and never returns leaks a row, and something that does it in a loop is a
/// memory exhaustion this server hands out for free. The oldest go first.
const MAX_SESSIONS: usize = 1024;

/// How long a session outlives its last use, in milliseconds.
///
/// An hour. Long enough that an agent idle between turns is not logged out
/// mid-conversation, short enough that a day of dead handshakes is not still
/// resident. A client that comes back later is told 404 and handshakes again,
/// which costs it one round trip and no state.
const IDLE_MS: Timestamp = 60 * 60 * 1000;

/// What `initialize` settled, kept until the client stops asking.
pub struct Sessions {
    live: Mutex<HashMap<String, Entry>>,
}

struct Entry {
    handshake: Handshake,
    /// When this was last used, for eviction. Refreshed on every lookup, so
    /// the timeout is idleness rather than age -- a session in steady use is
    /// never collected out from under a working client.
    touched: Timestamp,
}

impl Default for Sessions {
    fn default() -> Self {
        Self::new()
    }
}

impl Sessions {
    pub fn new() -> Self {
        Sessions {
            live: Mutex::new(HashMap::new()),
        }
    }

    /// Record a handshake and return the id that will restore it.
    pub fn mint(&self, handshake: Handshake, now: Timestamp) -> String {
        let id = mint_id(now);
        let mut live = self.lock();
        evict(&mut live, now);
        live.insert(
            id.clone(),
            Entry {
                handshake,
                touched: now,
            },
        );
        id
    }

    /// What the named session settled, if it is still live.
    ///
    /// `None` is the 404 case: an id this server did not mint, or one it has
    /// since dropped. The two are deliberately indistinguishable -- the client
    /// does the same thing either way, which is handshake again.
    pub fn resume(&self, id: &str, now: Timestamp) -> Option<Handshake> {
        let mut live = self.lock();
        let entry = live.get_mut(id)?;
        if !expired(entry, now) {
            entry.touched = now;
            return Some(entry.handshake.clone());
        }
        // Expired, and dropped here rather than left for the next `mint`.
        // Checking on the read is what makes the timeout a property of the
        // clock: eviction alone runs only when something is minted, so on a
        // server nobody new is handshaking against, a session idle for a day
        // would still answer. The hour is what this module says it is.
        live.remove(id);
        None
    }

    /// End a session on the client's say-so. `false` if it was not there.
    pub fn end(&self, id: &str) -> bool {
        self.lock().remove(id).is_some()
    }

    /// How many are live. For the tests, and for nothing else.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.lock().len()
    }

    /// A poisoned lock is not a reason to stop serving.
    ///
    /// Nothing here has invariants that a panic mid-write could break: the map
    /// is inserts and removes of independent rows. Refusing every later
    /// request because one connection thread panicked would turn a single
    /// failed request into a dead server.
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.live.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Whether an entry has gone untouched for longer than the timeout.
///
/// One definition with two callers -- [`Sessions::resume`] on the read and
/// [`evict`] on the write -- because two copies of this comparison are two
/// places for the boundary to drift, and a session that one considers live and
/// the other does not is the bug this exists to prevent.
fn expired(entry: &Entry, now: Timestamp) -> bool {
    now.saturating_sub(entry.touched) >= IDLE_MS
}

/// Drop what has expired, then the oldest, until the table fits.
///
/// The bound, not the timeout: `resume` is what enforces the hour, and this
/// keeps the table from growing without one. A session nobody ever returns to
/// is dropped here and never read either way.
fn evict(live: &mut HashMap<String, Entry>, now: Timestamp) {
    live.retain(|_, e| !expired(e, now));
    // `>=`, because this runs before an insert and the room has to exist
    // afterwards.
    while live.len() >= MAX_SESSIONS {
        let Some(oldest) = live
            .iter()
            .min_by_key(|(_, e)| e.touched)
            .map(|(id, _)| id.clone())
        else {
            return;
        };
        live.remove(&oldest);
    }
}

/// 128 bits, hex, from a key the operating system chose.
///
/// See the module comment for why this and not a CSPRNG. `now` is in here to
/// vary the input rather than to supply entropy: two mints in the same
/// millisecond get different ids because the keys differ, not because the
/// timestamps do.
fn mint_id(now: Timestamp) -> String {
    let seed = RandomState::new();
    let hi = seed.hash_one(now);
    // A second block under the same key. The constant is the golden-ratio odd
    // integer, which is only here to make the two inputs differ.
    let lo = seed.hash_one(hi ^ 0x9E37_79B9_7F4A_7C15);
    format!("{hi:016x}{lo:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hs(name: &str) -> Handshake {
        Handshake {
            client: Some(name.to_string()),
            negotiated: Some("2025-06-18".into()),
        }
    }

    #[test]
    fn a_minted_session_comes_back_and_an_unknown_one_does_not() {
        let s = Sessions::new();
        let id = s.mint(hs("agent-a"), 1_000);
        assert_eq!(s.resume(&id, 1_001), Some(hs("agent-a")));
        assert_eq!(s.resume("not an id", 1_001), None);
    }

    /// Two clients against one server keep their own names. This is the whole
    /// point of the module: a shared log that cannot tell them apart is the
    /// state `http.rs` used to be in.
    #[test]
    fn two_sessions_do_not_borrow_each_others_identity() {
        let s = Sessions::new();
        let a = s.mint(hs("agent-a"), 1_000);
        let b = s.mint(hs("agent-b"), 1_000);
        assert_ne!(a, b, "ids collided");
        assert_eq!(
            s.resume(&a, 1_001).and_then(|h| h.client).as_deref(),
            Some("agent-a")
        );
        assert_eq!(
            s.resume(&b, 1_001).and_then(|h| h.client).as_deref(),
            Some("agent-b")
        );
    }

    #[test]
    fn a_session_ends_when_the_client_says_so() {
        let s = Sessions::new();
        let id = s.mint(hs("agent-a"), 1_000);
        assert!(s.end(&id));
        assert_eq!(s.resume(&id, 1_001), None);
        assert!(!s.end(&id), "ending twice is not an error, but it is false");
    }

    /// Idleness, not age: a session in steady use outlives the timeout.
    #[test]
    fn use_keeps_a_session_alive_and_silence_does_not() {
        let s = Sessions::new();
        let busy = s.mint(hs("busy"), 0);
        let idle = s.mint(hs("idle"), 0);

        // Touched every half hour across three hours.
        let mut t = 0;
        for _ in 0..6 {
            t += IDLE_MS / 2;
            assert!(
                s.resume(&busy, t).is_some(),
                "collected while in use at {t}"
            );
        }

        assert_eq!(
            s.resume(&idle, t),
            None,
            "an idle session outlived its timeout"
        );
    }

    /// The timeout is the clock's, not the traffic's.
    ///
    /// This is the case the first version of this module got wrong: `evict`
    /// runs only from `mint`, so a server nobody new handshakes against never
    /// collected anything, and a session idle for a day still answered. The
    /// test above passed anyway because it minted to force the sweep -- which
    /// was the tell, since a timeout needing unrelated traffic to fire is not
    /// the hour this module promises.
    ///
    /// Nothing is minted here after the first, deliberately.
    #[test]
    fn a_quiet_server_still_expires_its_sessions() {
        let s = Sessions::new();
        let id = s.mint(hs("agent-a"), 0);

        assert!(
            s.resume(&id, IDLE_MS - 1).is_some(),
            "expired a millisecond early"
        );
        // Touched at IDLE_MS - 1 above, so the hour runs from there.
        assert_eq!(s.resume(&id, IDLE_MS * 24), None, "answered a day late");
        assert_eq!(s.len(), 0, "an expired session was left in the table");
    }

    /// The boundary is one comparison, so the two paths cannot disagree about
    /// which side of it a session falls on.
    #[test]
    fn the_read_and_the_sweep_agree_on_what_expired_means() {
        for (idle_for, still_live) in [(0, true), (IDLE_MS - 1, true), (IDLE_MS, false)] {
            // The read path.
            let s = Sessions::new();
            let id = s.mint(hs("a"), 0);
            assert_eq!(
                s.resume(&id, idle_for).is_some(),
                still_live,
                "resume disagreed at {idle_for}"
            );

            // The sweep, reached by minting something else.
            let s = Sessions::new();
            let id = s.mint(hs("a"), 0);
            s.mint(hs("b"), idle_for);
            assert_eq!(
                s.resume(&id, idle_for).is_some(),
                still_live,
                "evict disagreed at {idle_for}"
            );
        }
    }

    #[test]
    fn the_table_is_bounded_and_drops_the_oldest_first() {
        let s = Sessions::new();
        // All within the idle window, so only the count can evict.
        let first = s.mint(hs("first"), 1);
        for i in 0..MAX_SESSIONS {
            s.mint(hs("filler"), 2 + i as Timestamp);
        }
        assert!(s.len() <= MAX_SESSIONS, "grew to {}", s.len());
        assert_eq!(
            s.resume(&first, 3),
            None,
            "the oldest should have gone first"
        );
    }

    /// Not a randomness test -- it cannot be, at this sample size. It catches
    /// the mistake that would actually happen: an id derived from the clock
    /// alone, which collides for everything minted in the same millisecond.
    #[test]
    fn ids_minted_in_one_millisecond_differ() {
        let s = Sessions::new();
        let ids: std::collections::HashSet<String> =
            (0..256).map(|_| s.mint(hs("a"), 1_000)).collect();
        assert_eq!(ids.len(), 256, "ids collided within one millisecond");
        assert!(
            ids.iter().all(|i| i.len() == 32),
            "an id is 128 bits of hex"
        );
    }
}
