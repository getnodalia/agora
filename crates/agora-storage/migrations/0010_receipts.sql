-- Delivery receipts: the newest message of a channel that reached one of a member's devices.
-- Reading implies delivery, so existing members start delivered up to what they have read.
ALTER TABLE channel_members ADD COLUMN delivered_id INTEGER NOT NULL DEFAULT 0;
UPDATE channel_members SET delivered_id = last_read_id;
