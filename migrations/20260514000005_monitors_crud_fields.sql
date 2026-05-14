ALTER TABLE monitors
    ADD COLUMN status             TEXT NOT NULL DEFAULT 'active'
        CHECK (status IN ('active', 'paused', 'archived')),
    ADD COLUMN failure_threshold  INT  NOT NULL DEFAULT 1
        CHECK (failure_threshold > 0),
    ADD COLUMN recovery_threshold INT  NOT NULL DEFAULT 1
        CHECK (recovery_threshold > 0),
    ADD COLUMN kind               TEXT NOT NULL DEFAULT 'uptime';
