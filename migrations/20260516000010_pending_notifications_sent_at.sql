ALTER TABLE pending_notifications ADD COLUMN sent_at TIMESTAMPTZ;

CREATE INDEX pending_notifications_unsent_idx
    ON pending_notifications (created_at)
    WHERE sent_at IS NULL;
