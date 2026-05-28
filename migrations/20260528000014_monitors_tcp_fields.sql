ALTER TABLE monitors
    ALTER COLUMN url DROP NOT NULL,
    ADD COLUMN tcp_host TEXT,
    ADD COLUMN tcp_port INT
        CHECK (tcp_port IS NULL OR (tcp_port >= 1 AND tcp_port <= 65535)),
    ADD CONSTRAINT monitors_target_check CHECK (
        (kind = 'uptime' AND url IS NOT NULL)
        OR (kind = 'tcp' AND tcp_host IS NOT NULL AND tcp_port IS NOT NULL)
    );
