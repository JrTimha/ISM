CREATE TYPE msg_type AS ENUM ('Text', 'Media', 'RoomChange', 'Reply');

CREATE TABLE chat_message
(
    message_id   UUID                        NOT NULL PRIMARY KEY,
    chat_room_id UUID                        NOT NULL REFERENCES chat_room (id),
    sender_id    UUID                        NOT NULL,
    msg_body     JSONB                       NOT NULL,
    msg_type     msg_type                    NOT NULL,
    created_at   TIMESTAMP(6) WITH TIME ZONE NOT NULL
);

CREATE INDEX idx_chat_message_room_timeline ON chat_message (chat_room_id, created_at DESC);