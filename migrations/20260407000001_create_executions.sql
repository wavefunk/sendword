CREATE TABLE executions (
    id              TEXT PRIMARY KEY NOT NULL,
    hook_slug       TEXT NOT NULL,
    triggered_at    TEXT NOT NULL,
    started_at      TEXT,
    completed_at    TEXT,
    status          TEXT NOT NULL DEFAULT 'pending'
                    CHECK (status IN (
                        'pending', 'pending_approval', 'approved', 'rejected',
                        'expired', 'running', 'success', 'failed', 'timed_out'
                    )),
    exit_code       INTEGER,
    log_path        TEXT NOT NULL,
    trigger_source  TEXT NOT NULL,
    request_payload TEXT NOT NULL DEFAULT '{}',
    retry_count     INTEGER NOT NULL DEFAULT 0,
    retry_of        TEXT REFERENCES executions(id),
    approved_at     TEXT,
    approved_by     TEXT
);

CREATE INDEX idx_executions_hook_slug ON executions(hook_slug);
CREATE INDEX idx_executions_status ON executions(status);
CREATE INDEX idx_executions_triggered_at ON executions(triggered_at);
CREATE INDEX idx_executions_hook_triggered ON executions(hook_slug, triggered_at DESC);
