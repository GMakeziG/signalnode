ALTER TABLE monitors
    ADD COLUMN last_checked_at TIMESTAMPTZ NULL;

CREATE INDEX monitors_active_due_idx
    ON monitors (last_checked_at ASC NULLS FIRST)
    WHERE status = 'active';
