ALTER TABLE chat_room
    ALTER COLUMN latest_message_preview_text TYPE jsonb
    USING latest_message_preview_text::jsonb;