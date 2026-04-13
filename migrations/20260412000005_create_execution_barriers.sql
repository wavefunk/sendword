CREATE TABLE execution_locks (
    hook_slug    TEXT PRIMARY KEY NOT NULL,
    execution_id TEXT NOT NULL,
    acquired_at  TEXT NOT NULL
);

CREATE TABLE execution_queue (
    id           TEXT PRIMARY KEY NOT NULL,
    hook_slug    TEXT NOT NULL,
    execution_id TEXT NOT NULL REFERENCES executions(id),
    position     INTEGER NOT NULL,
    queued_at    TEXT NOT NULL,
    status       TEXT NOT NULL DEFAULT 'waiting'
                 CHECK (status IN ('waiting', 'ready', 'expired')),
    UNIQUE (hook_slug, position)
);

CREATE INDEX idx_execution_queue_hook_status ON execution_queue(hook_slug, status, position);
