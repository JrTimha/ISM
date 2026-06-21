-- Restore the participant_state column, its CHECK constraint and the state-based index.
-- Note: rows deleted by the up-migration (former Left/Invited members) cannot be recovered.
ALTER TABLE chat_room_participant
    ADD COLUMN participant_state varchar(255) NOT NULL DEFAULT 'Joined'
        CONSTRAINT chat_room_participant_participant_state_check
            CHECK ((participant_state)::text = ANY
                ((ARRAY ['Joined'::character varying, 'Invited'::character varying, 'Left'::character varying])::text[]));

ALTER TABLE chat_room_participant
    ALTER COLUMN participant_state DROP DEFAULT;

CREATE INDEX idx_participants_room_id_membership
    ON chat_room_participant (room_id, participant_state);
