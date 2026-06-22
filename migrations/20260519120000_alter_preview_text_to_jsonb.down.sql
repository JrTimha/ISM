ALTER TABLE chat_room
    ALTER COLUMN latest_message_preview_text TYPE varchar(255)
    USING latest_message_preview_text::text;