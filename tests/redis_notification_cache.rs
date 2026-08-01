//! The notification replay cache, against a **running** Redis.
//!
//! The sequencing and replay rules live in Lua inside Redis and in the exact semantics of `XADD`
//! with an explicit entry ID. An in-memory fake can mirror the rules, and `src/cache/test_support.rs`
//! does, but it cannot tell you whether the script is the rules — whether `XADD` accepts the ID the
//! script builds, whether the counter and the stream stay in step when one of them is lost, whether
//! a trimmed window really produces the resync the code believes it produces. That needs Redis.
//!
//! # Running it
//!
//! ```sh
//! docker compose up -d redis
//! ISM_TEST_REDIS_URL=redis://127.0.0.1:6379 cargo test --test redis_notification_cache -- --nocapture
//! ```
//!
//! Without `ISM_TEST_REDIS_URL` every test **skips**, so `cargo test` stays green on a machine with
//! no stack. Set `ISM_TEST_STRICT=1` to turn every skip into a failure — do that in CI, otherwise a
//! suite that silently tested nothing looks exactly like a suite that passed.
//!
//! Each test works under a fresh `Uuid`, so runs never collide, and deletes both of its keys
//! afterwards.

#![allow(clippy::expect_used)]

use ism::broadcast::{Notification, NotificationEvent};
use ism::cache::redis_cache::{Cache, RedisCache, ReplayResult};
use redis::AsyncTypedCommands;
use redis::aio::ConnectionManager;
use uuid::Uuid;

/// Decoded `XRANGE` reply: a list of `(entry_id, [(field, value), ...])`.
type StreamEntries = Vec<(String, Vec<(String, String)>)>;

/// Reports that a test could not run. Fails instead when `ISM_TEST_STRICT` is set.
fn skip(reason: &str) {
    assert!(std::env::var("ISM_TEST_STRICT").is_err(), "SKIPPED under ISM_TEST_STRICT: {reason}");
    println!("SKIPPED: {reason}");
}

/// Connects, or `None` when there is no Redis to connect to.
///
/// Returns the raw [`ConnectionManager`] alongside the cache: half of what these tests assert is
/// about the stored representation — entry IDs, the absence of `seq` in the payload — which the
/// `Cache` trait deliberately does not expose.
async fn connect() -> Option<(RedisCache, ConnectionManager)> {
    let url = match std::env::var("ISM_TEST_REDIS_URL") {
        Ok(url) if !url.is_empty() => url,
        _ => {
            skip("ISM_TEST_REDIS_URL is not set");
            return None;
        }
    };

    match RedisCache::connect(url.clone()).await {
        Ok(connection) => {
            let raw = connection.cache.connection.clone();
            Some((connection.cache, raw))
        }
        Err(error) => {
            skip(&format!("Redis at {url} is unreachable: {error}"));
            None
        }
    }
}

/// Runs `body` against a fresh user, then removes both of that user's keys.
///
/// A macro rather than a function taking a closure: an async closure borrowing the connection is
/// more ceremony than the cleanup is worth, and every test needs the same three lines.
macro_rules! with_redis {
    (|$cache:ident, $con:ident, $user:ident| $body:block) => {{
        let Some((cache, mut raw)) = connect().await else { return };
        #[allow(unused_mut)]
        let (mut $cache, mut $con, $user) = (cache, raw.clone(), Uuid::new_v4());

        $body

        let _ = raw.del(&[seq_key(&$user), stream_key(&$user)]).await;
    }};
}

fn seq_key(user_id: &Uuid) -> String {
    format!("user_seq:{}", user_id)
}

fn stream_key(user_id: &Uuid) -> String {
    format!("user_notifications:{}", user_id)
}

/// A durable event. `UserReadChat` is the cheapest one to build and the payload is not what any of
/// these tests is about.
fn event() -> Notification {
    Notification::new(NotificationEvent::UserReadChat {
        user_id: Uuid::new_v4(),
        room_id: Uuid::new_v4(),
    })
}

async fn entries(con: &mut ConnectionManager, user_id: &Uuid) -> StreamEntries {
    redis::cmd("XRANGE")
        .arg(stream_key(user_id))
        .arg("-")
        .arg("+")
        .query_async(con)
        .await
        .expect("XRANGE")
}

#[tokio::test]
async fn appends_allocate_monotonic_sequences() {
    with_redis!(|cache, con, user| {
        let mut assigned = Vec::new();
        for _ in 0..5 {
            assigned.push(cache.append_notification(&user, &event()).await.expect("append"));
        }

        assert_eq!(assigned, (1..=5).map(Some).collect::<Vec<_>>());
        assert_eq!(cache.current_sequence(&user).await.expect("current"), Some(5));
        assert_eq!(entries(&mut con, &user).await.len(), 5);
    });
}

/// The sequence is stored once, in the entry ID, and re-attached on read. If it were also written
/// into the payload the two could disagree; this is what makes that unrepresentable.
#[tokio::test]
async fn the_stored_payload_omits_the_sequence_and_replay_restores_it() {
    with_redis!(|cache, con, user| {
        cache.append_notification(&user, &event()).await.expect("append");

        let stored = entries(&mut con, &user).await;
        assert_eq!(stored.len(), 1);

        let (id, fields) = &stored[0];
        assert_eq!(id, "1-0", "the entry ID is the sequence");

        let (_, payload) = fields.iter().find(|(field, _)| field == "data").expect("data field");
        assert!(!payload.contains("\"seq\""), "the stored payload must not carry a sequence: {payload}");

        match cache.get_notifications_since_seq(&user, 0).await.expect("replay") {
            ReplayResult::Events(events) => {
                assert_eq!(events.len(), 1);
                assert_eq!(events[0].seq, Some(1), "replay must restore the sequence from the entry ID");
            }
            ReplayResult::ResyncNeeded => panic!("expected the event to be replayable"),
        }
    });
}

/// The property the two-round-trip write path could not offer: whatever the interleaving, the
/// sequences issued are exactly the entries stored. A sequence allocated and then not stored would
/// show up here as a hole.
#[tokio::test]
async fn concurrent_appends_leave_no_gap_and_no_duplicate() {
    with_redis!(|cache, con, user| {
        let events: Vec<Notification> = (0..50).map(|_| event()).collect();
        let appends = events.iter().map(|event| cache.append_notification(&user, event));
        let mut assigned: Vec<u64> = futures::future::join_all(appends)
            .await
            .into_iter()
            .map(|result| result.expect("append").expect("sequencing available"))
            .collect();
        assigned.sort_unstable();

        assert_eq!(assigned, (1..=50).collect::<Vec<_>>());

        let stored: Vec<String> = entries(&mut con, &user).await.into_iter().map(|(id, _)| id).collect();
        assert_eq!(stored, (1..=50).map(|seq| format!("{seq}-0")).collect::<Vec<_>>());
    });
}

/// The bug the atomic script exists for.
///
/// `XADD` rejects any explicit ID that is not strictly greater than the stream's
/// `last-generated-id`, and that value survives trimming — so if the counter is lost while the
/// stream is not, restarting it at 1 makes every subsequent write fail permanently, until the
/// stream's own TTL expires. Under `maxmemory` the tiny counter is a far more attractive eviction
/// candidate than the multi-KB stream, so this is a live failure mode, not a thought experiment.
///
/// Against the previous write path both halves of this fail with *"The ID specified in XADD is
/// equal or smaller than the target stream top item"*.
#[tokio::test]
async fn a_lost_counter_realigns_onto_the_surviving_stream() {
    with_redis!(|cache, con, user| {
        for _ in 0..5 {
            cache.append_notification(&user, &event()).await.expect("append");
        }

        con.del(seq_key(&user)).await.expect("drop the counter");
        let next = cache.append_notification(&user, &event()).await.expect("append after a lost counter");
        assert_eq!(next, Some(6), "the sequence must resume above the stream, not restart at 1");

        // Again with an empty stream: `last-generated-id` outlives the entries themselves, so
        // "the stream is empty" is not enough to conclude that `1-0` is free.
        redis::cmd("XTRIM")
            .arg(stream_key(&user))
            .arg("MAXLEN")
            .arg(0)
            .query_async::<()>(&mut con)
            .await
            .expect("XTRIM");
        con.del(seq_key(&user)).await.expect("drop the counter again");

        let next = cache.append_notification(&user, &event()).await.expect("append onto a trimmed stream");
        assert_eq!(next, Some(7), "an emptied stream still remembers its last generated ID");
    });
}

#[tokio::test]
async fn a_gap_beyond_the_retained_window_needs_a_resync() {
    with_redis!(|cache, con, user| {
        for _ in 0..20 {
            cache.append_notification(&user, &event()).await.expect("append");
        }

        // Exact, not `~`: an approximate trim is free to keep more than asked.
        redis::cmd("XTRIM")
            .arg(stream_key(&user))
            .arg("MAXLEN")
            .arg(5)
            .query_async::<()>(&mut con)
            .await
            .expect("XTRIM");

        let replay = cache.get_notifications_since_seq(&user, 1).await.expect("replay");
        assert!(matches!(replay, ReplayResult::ResyncNeeded), "a trimmed-away gap cannot be replayed losslessly");

        // A client inside the retained window is still served normally.
        match cache.get_notifications_since_seq(&user, 17).await.expect("replay") {
            ReplayResult::Events(events) => assert_eq!(events.iter().filter_map(|event| event.seq).collect::<Vec<_>>(), vec![18, 19, 20]),
            ReplayResult::ResyncNeeded => panic!("seq 17 is inside the retained window"),
        }
    });
}

#[tokio::test]
async fn a_cursor_ahead_of_the_counter_needs_a_resync() {
    with_redis!(|cache, _con, user| {
        let replay = cache.get_notifications_since_seq(&user, 999).await.expect("replay");
        assert!(
            matches!(replay, ReplayResult::ResyncNeeded),
            "a cursor above the counter means the sequence space was reset"
        );
    });
}

/// An entry we cannot decode is a lost event, not a skippable one: the caller derives its
/// high-water mark from what it received, so dropping the entry silently would advance the client's
/// cursor past an event it never got. This goes live the moment the envelope format changes while a
/// user's stream still holds older entries.
#[tokio::test]
async fn an_undecodable_entry_needs_a_resync() {
    with_redis!(|cache, con, user| {
        cache.append_notification(&user, &event()).await.expect("append");

        redis::cmd("XADD")
            .arg(stream_key(&user))
            .arg("2-0")
            .arg("data")
            .arg("{ not json")
            .query_async::<String>(&mut con)
            .await
            .expect("XADD");

        let replay = cache.get_notifications_since_seq(&user, 0).await.expect("replay");
        assert!(
            matches!(replay, ReplayResult::ResyncNeeded),
            "an undecodable entry must not be silently skipped"
        );
    });
}
