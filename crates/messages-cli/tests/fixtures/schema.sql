CREATE TABLE chat (guid TEXT NOT NULL UNIQUE, display_name TEXT, service_name TEXT);
CREATE TABLE handle (id TEXT NOT NULL);
CREATE TABLE message (
    guid TEXT NOT NULL UNIQUE, text TEXT, attributedBody BLOB, date INTEGER,
    handle_id INTEGER DEFAULT 0, is_from_me INTEGER DEFAULT 0,
    is_read INTEGER DEFAULT 0, is_finished INTEGER DEFAULT 1,
    item_type INTEGER DEFAULT 0, associated_message_guid TEXT,
    associated_message_type INTEGER DEFAULT 0
);
CREATE TABLE chat_message_join (chat_id INTEGER, message_id INTEGER, message_date INTEGER, UNIQUE(chat_id,message_id));
CREATE TABLE chat_handle_join (chat_id INTEGER, handle_id INTEGER, UNIQUE(chat_id,handle_id));
CREATE TABLE attachment (guid TEXT NOT NULL UNIQUE, filename TEXT, transfer_name TEXT, mime_type TEXT, total_bytes INTEGER);
CREATE TABLE message_attachment_join (message_id INTEGER, attachment_id INTEGER, UNIQUE(message_id,attachment_id));
CREATE INDEX message_idx_date ON message(date);
CREATE INDEX chat_message_date ON chat_message_join(chat_id,message_date,message_id);
CREATE INDEX chat_message_id ON chat_message_join(message_id);
CREATE INDEX chat_handle_id ON chat_handle_join(handle_id);
CREATE INDEX message_attachment_id ON message_attachment_join(message_id);
