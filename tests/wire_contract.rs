//! Golden snapshots of every byte ISM puts on a wire.
//!
//! This file exists to make "the data-model refactor changed no payload" a fact the test suite
//! proves rather than a claim the diff asserts. It covers two distinct contracts that the old
//! model layer welded into single types:
//!
//! 1. **HTTP / streaming responses** — what a client parses.
//! 2. **JSONB column payloads** — what is already written in `chat_message.msg_body` and
//!    `chat_room.latest_message_preview_text`. Changing one of these shapes does not break a
//!    client; it breaks decoding of every row already in the database, which is worse.
//!
//! **The expected JSON literals below are the contract. A refactor may rename the Rust types in
//! this file; it may not edit a single literal.** If an assertion fails, the change broke a
//! contract — fix the change, not the expectation.
//!
//! Values are fixed (no `Utc::now()`, no `Uuid::new_v4()`) so the snapshots are deterministic.
//! `Notification` is built by struct literal rather than `Notification::new`, which stamps the
//! current time — the production rule against constructing it directly is about broadcast call
//! sites, not about a test that needs a pinned `createdAt`.

use chrono::{DateTime, Utc};
use ism::broadcast::{Notification, NotificationEvent};
use ism::core::cursor::CursorResults;
use ism::messaging::entity::{MediaJson, MessageBodyJson, RepliedMessageJson, ReplyJson, RoomChangeJson, TextJson};
use ism::messaging::model::MsgType;
use ism::messaging::response::{MessageBodyResponse, MessageResponse, TextBodyResponse, TimelinePageResponse};
use ism::rooms::entity::{LastMessagePreviewJson, RoomMemberSnapshotJson};
use ism::rooms::model::{RoomChangeType, RoomContext, RoomType};
use ism::rooms::response::{
    LastMessagePreviewResponse, RoomDetailResponse, RoomImageUploadResponse, RoomMemberResponse, RoomResponse, ShareTargetRef, ShareTargetResponse,
};
use ism::users::response::{Relationship, RelationshipStateResponse, UserProfileResponse, UserWithRelationshipResponse};
use serde_json::{Value, json};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const USER_A: &str = "11111111-1111-4111-8111-111111111111";
const USER_B: &str = "22222222-2222-4222-8222-222222222222";
const ROOM_ID: &str = "33333333-3333-4333-8333-333333333333";
const MSG_ID: &str = "44444444-4444-4444-8444-444444444444";
const REPLY_ID: &str = "55555555-5555-4555-8555-555555555555";

const TS: &str = "2026-01-15T12:30:45Z";
const TS2: &str = "2026-01-15T13:00:00Z";

fn uuid(s: &str) -> Uuid {
    Uuid::parse_str(s).expect("fixture uuid is valid")
}

fn ts(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).expect("fixture timestamp is valid rfc3339").with_timezone(&Utc)
}

fn user() -> UserProfileResponse {
    UserProfileResponse {
        id: uuid(USER_A),
        display_name: "Ada".to_string(),
        street_credits: 42,
        profile_picture: Some("https://cdn.example/a.png".to_string()),
        description: Some("builds things".to_string()),
        friends_count: 12,
        posts_count: 7,
        role: "USER".to_string(),
    }
}

fn user_json() -> Value {
    json!({
        "id": USER_A,
        "displayName": "Ada",
        "streetCredits": 42,
        "profilePicture": "https://cdn.example/a.png",
        "description": "builds things",
        "friendsCount": 12,
        "postsCount": 7,
        "role": "USER"
    })
}

fn member() -> RoomMemberResponse {
    RoomMemberResponse {
        id: uuid(USER_A),
        display_name: "Ada".to_string(),
        profile_picture: Some("https://cdn.example/a.png".to_string()),
        joined_at: Some(ts(TS)),
        last_message_read_at: None,
    }
}

fn member_json() -> Value {
    json!({
        "id": USER_A,
        "displayName": "Ada",
        "profilePicture": "https://cdn.example/a.png",
        "joinedAt": TS,
        "lastMessageReadAt": null
    })
}

fn member_snapshot() -> RoomMemberSnapshotJson {
    RoomMemberSnapshotJson {
        id: uuid(USER_A),
        display_name: "Ada".to_string(),
        profile_picture: Some("https://cdn.example/a.png".to_string()),
        joined_at: Some(ts(TS)),
        last_message_read_at: None,
    }
}

fn preview() -> LastMessagePreviewResponse {
    LastMessagePreviewResponse::Text {
        sender_username: "Ada".to_string(),
        text: "hello there".to_string(),
    }
}

fn preview_json() -> Value {
    json!({ "type": "Text", "sender_username": "Ada", "text": "hello there" })
}

fn room() -> RoomResponse {
    RoomResponse {
        id: uuid(ROOM_ID),
        room_type: RoomType::Single,
        room_image_url: None,
        room_name: Some("Team".to_string()),
        created_at: ts(TS),
        latest_message: Some(ts(TS2)),
        unread: Some(true),
        latest_message_preview_text: preview(),
    }
}

fn room_json() -> Value {
    json!({
        "id": ROOM_ID,
        "roomType": "Single",
        "roomImageUrl": null,
        "roomName": "Team",
        "createdAt": TS,
        "latestMessage": TS2,
        "unread": true,
        "latestMessagePreviewText": preview_json()
    })
}

fn message() -> MessageResponse {
    MessageResponse {
        chat_room_id: uuid(ROOM_ID),
        message_id: uuid(MSG_ID),
        sender_id: uuid(USER_A),
        msg_body: MessageBodyResponse::Text(TextBodyResponse {
            text: "hello there".to_string(),
        }),
        msg_type: MsgType::Text,
        created_at: ts(TS),
    }
}

fn message_json() -> Value {
    json!({
        "chatRoomId": ROOM_ID,
        "messageId": MSG_ID,
        "senderId": USER_A,
        "msgBody": { "text": "hello there" },
        "msgType": "Text",
        "createdAt": TS
    })
}

fn notification(seq: Option<u64>, body: NotificationEvent) -> Notification {
    Notification {
        v: 1,
        seq,
        body,
        created_at: ts(TS),
    }
}

#[track_caller]
fn assert_wire<T: serde::Serialize>(value: &T, expected: Value) {
    assert_eq!(serde_json::to_value(value).expect("value serializes"), expected);
}

// ---------------------------------------------------------------------------
// HTTP responses — users
// ---------------------------------------------------------------------------

#[test]
fn user_profile_wire() {
    assert_wire(&user(), user_json());
}

#[test]
fn user_with_relationship_wire() {
    let dto = UserWithRelationshipResponse {
        user: user(),
        relationship_type: Some(Relationship::Friend),
    };
    assert_wire(&dto, json!({ "user": user_json(), "relationshipType": "FRIEND" }));
}

#[test]
fn user_with_relationship_wire_without_relationship() {
    let dto = UserWithRelationshipResponse {
        user: user(),
        relationship_type: None,
    };
    assert_wire(&dto, json!({ "user": user_json(), "relationshipType": null }));
}

#[test]
fn relationship_variants_wire() {
    for (variant, expected) in [
        (Relationship::Friend, "FRIEND"),
        (Relationship::InviteSent, "INVITE_SENT"),
        (Relationship::InviteReceived, "INVITE_RECEIVED"),
        (Relationship::ClientBlocked, "CLIENT_BLOCKED"),
        (Relationship::ClientGotBlocked, "CLIENT_GOT_BLOCKED"),
    ] {
        assert_wire(&RelationshipStateResponse { state: Some(variant) }, json!({ "state": expected }));
    }
}

#[test]
fn relationship_state_response_empty_wire() {
    assert_wire(&RelationshipStateResponse { state: None }, json!({ "state": null }));
}

// ---------------------------------------------------------------------------
// HTTP responses — rooms
// ---------------------------------------------------------------------------

#[test]
fn room_wire() {
    assert_wire(&room(), room_json());
}

#[test]
fn room_member_wire() {
    assert_wire(&member(), member_json());
}

#[test]
fn room_with_users_flattens_the_room() {
    let dto = RoomDetailResponse {
        room: room(),
        users: vec![member()],
    };
    let mut expected = room_json();
    expected["users"] = json!([member_json()]);
    assert_wire(&dto, expected);
}

#[test]
fn upload_response_wire() {
    let dto = RoomImageUploadResponse {
        image_url: "https://cdn.example/room.png".to_string(),
        image_name: "room.png".to_string(),
    };
    assert_wire(&dto, json!({ "imageUrl": "https://cdn.example/room.png", "imageName": "room.png" }));
}

#[test]
fn room_type_wire() {
    assert_wire(&RoomType::Single, json!("Single"));
    assert_wire(&RoomType::Group, json!("Group"));
}

#[test]
fn share_target_room_wire() {
    // `ShareTargetRef` carries `rename_all = "camelCase"` at the enum level, which renames the
    // *variants* only — struct-variant fields keep their declared snake_case names. Pinned here
    // because it is the kind of asymmetry a refactor "tidies up" into a breaking change.
    let target = ShareTargetResponse {
        name: Some("Ada".to_string()),
        image_url: Some("https://cdn.example/a.png".to_string()),
        target: ShareTargetRef::Room {
            room_id: uuid(ROOM_ID),
            room_type: RoomType::Single,
        },
    };
    assert_wire(
        &target,
        json!({
            "name": "Ada",
            "imageUrl": "https://cdn.example/a.png",
            "target": { "kind": "room", "room_id": ROOM_ID, "room_type": "Single" }
        }),
    );
}

#[test]
fn share_target_user_wire() {
    let target = ShareTargetResponse {
        name: Some("Grace".to_string()),
        image_url: None,
        target: ShareTargetRef::User { user_id: uuid(USER_B) },
    };
    assert_wire(
        &target,
        json!({
            "name": "Grace",
            "imageUrl": null,
            "target": { "kind": "user", "user_id": USER_B }
        }),
    );
}

// ---------------------------------------------------------------------------
// HTTP responses — messaging
// ---------------------------------------------------------------------------

#[test]
fn message_wire() {
    assert_wire(&message(), message_json());
}

#[test]
fn timeline_page_wire() {
    let page = TimelinePageResponse {
        messages: vec![message()],
        senders: vec![member()],
    };
    assert_wire(&page, json!({ "messages": [message_json()], "senders": [member_json()] }));
}

#[test]
fn msg_type_wire() {
    assert_wire(&MsgType::Text, json!("Text"));
    assert_wire(&MsgType::Media, json!("Media"));
    assert_wire(&MsgType::Reply, json!("Reply"));
    assert_wire(&MsgType::RoomChange, json!("RoomChange"));
}

// ---------------------------------------------------------------------------
// HTTP responses — pagination envelope
// ---------------------------------------------------------------------------

#[test]
fn cursor_results_wire() {
    let page = CursorResults {
        cursor: Some("b3BhcXVl".to_string()),
        content: vec![user()],
    };
    assert_wire(&page, json!({ "cursor": "b3BhcXVl", "content": [user_json()] }));
}

#[test]
fn cursor_results_last_page_wire() {
    let page: CursorResults<UserProfileResponse> = CursorResults { cursor: None, content: vec![] };
    assert_wire(&page, json!({ "cursor": null, "content": [] }));
}

// ---------------------------------------------------------------------------
// Streaming envelope — one case per NotificationEvent variant
// ---------------------------------------------------------------------------

#[test]
fn notification_envelope_carries_version_and_seq() {
    let n = notification(
        Some(7),
        NotificationEvent::UserReadChat {
            user_id: uuid(USER_A),
            room_id: uuid(ROOM_ID),
        },
    );
    assert_wire(
        &n,
        json!({
            "v": 1,
            "seq": 7,
            "type": "UserReadChat",
            "userId": USER_A,
            "roomId": ROOM_ID,
            "createdAt": TS
        }),
    );
}

#[test]
fn notification_omits_absent_seq() {
    let n = notification(None, NotificationEvent::Resync { reason: "gap".to_string() });
    assert_wire(&n, json!({ "v": 1, "type": "Resync", "reason": "gap", "createdAt": TS }));
}

#[test]
fn friend_request_events_wire() {
    assert_wire(
        &notification(Some(1), NotificationEvent::FriendRequestReceived { from_user: user() }),
        json!({
            "v": 1, "seq": 1, "type": "FriendRequestReceived",
            "fromUser": user_json(), "createdAt": TS
        }),
    );
    assert_wire(
        &notification(Some(2), NotificationEvent::FriendRequestAccepted { from_user: user() }),
        json!({
            "v": 1, "seq": 2, "type": "FriendRequestAccepted",
            "fromUser": user_json(), "createdAt": TS
        }),
    );
}

#[test]
fn chat_message_event_wire() {
    let n = notification(
        Some(3),
        NotificationEvent::ChatMessage {
            message: message(),
            room_preview_text: preview(),
            sender: member(),
        },
    );
    assert_wire(
        &n,
        json!({
            "v": 1, "seq": 3, "type": "ChatMessage",
            "message": message_json(),
            "roomPreviewText": preview_json(),
            "sender": member_json(),
            "createdAt": TS
        }),
    );
}

#[test]
fn room_change_event_wire() {
    let n = notification(
        Some(4),
        NotificationEvent::RoomChangeEvent {
            message: message(),
            room_preview_text: preview(),
        },
    );
    assert_wire(
        &n,
        json!({
            "v": 1, "seq": 4, "type": "RoomChangeEvent",
            "message": message_json(),
            "roomPreviewText": preview_json(),
            "createdAt": TS
        }),
    );
}

#[test]
fn new_room_event_wire() {
    let n = notification(
        Some(5),
        NotificationEvent::NewRoom {
            room: room(),
            created_by: user(),
            first_message: Some(message()),
        },
    );
    assert_wire(
        &n,
        json!({
            "v": 1, "seq": 5, "type": "NewRoom",
            "room": room_json(),
            "createdBy": user_json(),
            "firstMessage": message_json(),
            "createdAt": TS
        }),
    );
}

#[test]
fn new_room_event_without_first_message_wire() {
    let n = notification(
        Some(6),
        NotificationEvent::NewRoom {
            room: room(),
            created_by: user(),
            first_message: None,
        },
    );
    assert_wire(
        &n,
        json!({
            "v": 1, "seq": 6, "type": "NewRoom",
            "room": room_json(),
            "createdBy": user_json(),
            "firstMessage": null,
            "createdAt": TS
        }),
    );
}

#[test]
fn leave_room_event_wire() {
    let n = notification(Some(8), NotificationEvent::LeaveRoom { room_id: uuid(ROOM_ID) });
    assert_wire(&n, json!({ "v": 1, "seq": 8, "type": "LeaveRoom", "roomId": ROOM_ID, "createdAt": TS }));
}

#[test]
fn system_message_event_wire() {
    let n = notification(
        Some(9),
        NotificationEvent::SystemMessage {
            message: json!({ "kind": "maintenance" }),
        },
    );
    assert_wire(
        &n,
        json!({
            "v": 1, "seq": 9, "type": "SystemMessage",
            "message": { "kind": "maintenance" },
            "createdAt": TS
        }),
    );
}

// ---------------------------------------------------------------------------
// Stored JSONB — chat_message.msg_body
//
// These are not an API. They are the on-disk format of rows that already exist, so a change
// here is a decoding failure for historical data, not a client-side parse error.
// ---------------------------------------------------------------------------

#[test]
fn stored_text_body() {
    let body = MessageBodyJson::Text(TextJson {
        text: "hello there".to_string(),
    });
    assert_wire(&body, json!({ "text": "hello there" }));
}

#[test]
fn stored_media_body() {
    let body = MessageBodyJson::Media(MediaJson {
        media_url: "https://cdn.example/pic.png".to_string(),
        media_type: "image".to_string(),
        mime_type: Some("image/png".to_string()),
        alt_text: Some("a cat".to_string()),
    });
    assert_wire(
        &body,
        json!({
            "mediaUrl": "https://cdn.example/pic.png",
            "mediaType": "image",
            "mimeType": "image/png",
            "altText": "a cat"
        }),
    );
}

#[test]
fn stored_reply_body() {
    let body = MessageBodyJson::Reply(ReplyJson {
        reply_msg_id: uuid(REPLY_ID),
        reply_sender_id: uuid(USER_B),
        reply_msg_type: MsgType::Text,
        reply_created_at: ts(TS),
        reply_msg_details: RepliedMessageJson::Text(TextJson { text: "original".to_string() }),
        reply_text: "answer".to_string(),
    });
    assert_wire(
        &body,
        json!({
            "replyMsgId": REPLY_ID,
            "replySenderId": USER_B,
            "replyMsgType": "Text",
            "replyCreatedAt": TS,
            "replyMsgDetails": { "text": "original" },
            "replyText": "answer"
        }),
    );
}

#[test]
fn stored_reply_to_a_reply_body() {
    let details = RepliedMessageJson::Reply {
        reply_text: "earlier answer".to_string(),
    };
    assert_wire(&details, json!({ "reply_text": "earlier answer" }));
}

#[test]
fn stored_room_change_body() {
    for (variant, tag) in [
        (
            RoomChangeJson::UserJoined {
                related_user: member_snapshot(),
            },
            "UserJoined",
        ),
        (
            RoomChangeJson::UserLeft {
                related_user: member_snapshot(),
            },
            "UserLeft",
        ),
        (
            RoomChangeJson::UserInvited {
                related_user: member_snapshot(),
            },
            "UserInvited",
        ),
    ] {
        assert_wire(&MessageBodyJson::RoomChange(variant), json!({ "type": tag, "related_user": member_json() }));
    }
}

// ---------------------------------------------------------------------------
// Stored JSONB — chat_room.latest_message_preview_text
//
// The storage and response halves are now separate types, and this is the section that proves the
// split fixed something rather than just renaming it: the stored value keeps the **full** text
// while the client still receives the **truncated** one. Before the split a single type did both
// jobs, so the display-only `serialize_with` also ran on the `INSERT` path and the database has
// been holding shortened previews.
//
// The response literals below are unchanged from the pre-split contract — only the type being
// constructed differs.
// ---------------------------------------------------------------------------

fn long_text() -> String {
    // 60 chars: over the 50-char threshold in utils::truncate_preview.
    "a".repeat(60)
}

fn truncated_text() -> String {
    format!("{}...", "a".repeat(40))
}

#[test]
fn stored_preview_keeps_the_full_text() {
    // The bug this refactor fixes: what goes into `chat_room.latest_message_preview_text` is now
    // the untruncated value, so the stored data is no longer shaped by a display rule.
    let stored = LastMessagePreviewJson::Text {
        sender_username: "Ada".to_string(),
        text: long_text(),
    };
    assert_wire(&stored, json!({ "type": "Text", "sender_username": "Ada", "text": long_text() }));

    let stored_reply = LastMessagePreviewJson::Reply {
        sender_username: "Ada".to_string(),
        reply_text: long_text(),
    };
    assert_wire(&stored_reply, json!({ "type": "Reply", "sender_username": "Ada", "reply_text": long_text() }));
}

#[test]
fn stored_preview_text_short_is_verbatim() {
    let p = LastMessagePreviewResponse::Text {
        sender_username: "Ada".to_string(),
        text: "hello there".to_string(),
    };
    assert_wire(&p, json!({ "type": "Text", "sender_username": "Ada", "text": "hello there" }));
}

#[test]
fn preview_response_truncates_long_text() {
    let response = LastMessagePreviewResponse::from(LastMessagePreviewJson::Text {
        sender_username: "Ada".to_string(),
        text: long_text(),
    });
    assert_wire(&response, json!({ "type": "Text", "sender_username": "Ada", "text": truncated_text() }));
}

#[test]
fn preview_response_truncates_long_reply_text() {
    let response = LastMessagePreviewResponse::from(LastMessagePreviewJson::Reply {
        sender_username: "Ada".to_string(),
        reply_text: long_text(),
    });
    assert_wire(&response, json!({ "type": "Reply", "sender_username": "Ada", "reply_text": truncated_text() }));
}

#[test]
fn truncation_is_idempotent_for_legacy_rows() {
    // Rows written before the fix already hold a shortened value: 43 chars, under the 50-char
    // threshold. Truncating it again is a no-op, so moving truncation to the read path leaves
    // historical previews byte-identical instead of eating another 3 characters each time.
    let legacy = truncated_text();
    assert_eq!(legacy.chars().count(), 43);
    let response = LastMessagePreviewResponse::from(LastMessagePreviewJson::Text {
        sender_username: "Ada".to_string(),
        text: legacy.clone(),
    });
    assert_wire(&response, json!({ "type": "Text", "sender_username": "Ada", "text": legacy }));
}

#[test]
fn stored_preview_media_and_room_change() {
    // Media and RoomChange carry no free text, so storage and response are identical; asserted on
    // both types to pin that the two enums have not drifted apart in tag or field names.
    for (stored, expected) in [
        (
            LastMessagePreviewJson::Media {
                sender_username: "Ada".to_string(),
                media_type: "image".to_string(),
            },
            json!({ "type": "Media", "sender_username": "Ada", "media_type": "image" }),
        ),
        (
            LastMessagePreviewJson::RoomChange {
                sender_username: "Ada".to_string(),
                room_change_type: RoomChangeType::JOIN,
            },
            json!({ "type": "RoomChange", "sender_username": "Ada", "room_change_type": "JOIN" }),
        ),
        (LastMessagePreviewJson::New, json!({ "type": "New" })),
    ] {
        assert_wire(&LastMessagePreviewResponse::from(stored.clone()), expected.clone());
        assert_wire(&stored, expected);
    }
}

// ---------------------------------------------------------------------------
// Redis cache shapes
//
// Not client-facing, but a mismatch here silently misses every `get_room_context` and degrades
// broadcast fan-out to a database read per event, so the format is pinned too.
// ---------------------------------------------------------------------------

#[test]
fn room_context_cache_shape() {
    let ctx = RoomContext { members: vec![member()] };
    assert_wire(&ctx, json!({ "members": [member_json()] }));
}

// ---------------------------------------------------------------------------
// Storage ↔ response equivalence
//
// The `…Json` and `…Response` families are separate types precisely so they *may* diverge. Today
// they must not: the whole refactor is premised on the payload being unchanged. This asserts the
// two serialize identically for every message body shape, so the moment someone deliberately
// evolves a response, this test fails and forces the change to be a conscious API decision with a
// migration note — rather than something noticed by a client.
// ---------------------------------------------------------------------------

#[test]
fn stored_and_response_bodies_agree_today() {
    let stored_bodies = [
        MessageBodyJson::Text(TextJson {
            text: "hello there".to_string(),
        }),
        MessageBodyJson::Media(MediaJson {
            media_url: "https://cdn.example/pic.png".to_string(),
            media_type: "image".to_string(),
            mime_type: Some("image/png".to_string()),
            alt_text: Some("a cat".to_string()),
        }),
        MessageBodyJson::Reply(ReplyJson {
            reply_msg_id: uuid(REPLY_ID),
            reply_sender_id: uuid(USER_B),
            reply_msg_type: MsgType::Text,
            reply_created_at: ts(TS),
            reply_msg_details: RepliedMessageJson::Text(TextJson { text: "original".to_string() }),
            reply_text: "answer".to_string(),
        }),
        MessageBodyJson::RoomChange(RoomChangeJson::UserJoined {
            related_user: member_snapshot(),
        }),
    ];

    for stored in stored_bodies {
        let stored_json = serde_json::to_value(&stored).expect("stored body serializes");
        let response = MessageBodyResponse::from(stored);
        let response_json = serde_json::to_value(&response).expect("response body serializes");
        assert_eq!(stored_json, response_json, "storage and response shapes have drifted");
    }
}

#[test]
fn stored_and_response_previews_agree_when_no_truncation_applies() {
    // Truncation is the one intended difference, so this uses short text: everything else about the
    // two preview shapes — tag, field names, variant names — must still match exactly.
    let stored = LastMessagePreviewJson::Text {
        sender_username: "Ada".to_string(),
        text: "short enough".to_string(),
    };
    let stored_json = serde_json::to_value(&stored).expect("stored preview serializes");
    let response_json = serde_json::to_value(LastMessagePreviewResponse::from(stored)).expect("response preview serializes");
    assert_eq!(stored_json, response_json);
}
