-- Leaving a room now deletes the participant row; there is no more "Left"/"Invited" state.
-- A row in chat_room_participant means "currently in the room".

-- 1. Drop historical non-joined rows so every remaining row is an active member.
DELETE FROM chat_room_participant WHERE participant_state <> 'Joined';

-- 2. Drop the state-based index, then the CHECK constraint and the column itself.
DROP INDEX IF EXISTS idx_participants_room_id_membership;

ALTER TABLE chat_room_participant
    DROP CONSTRAINT IF EXISTS chat_room_participant_participant_state_check;

ALTER TABLE chat_room_participant
    DROP COLUMN participant_state;
