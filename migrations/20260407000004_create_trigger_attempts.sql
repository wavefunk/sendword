CREATE TABLE trigger_attempts (
    id           TEXT PRIMARY KEY NOT NULL,
    hook_slug    TEXT NOT NULL,
    attempted_at TEXT NOT NULL,
    source_ip    TEXT NOT NULL,
    status       TEXT NOT NULL
                 CHECK (status IN (
                     'fired', 'auth_failed', 'validation_failed', 'filtered',
                     'rate_limited', 'schedule_skipped', 'cooldown_skipped',
                     'concurrency_rejected', 'pending_approval'
                 )),
    reason       TEXT NOT NULL DEFAULT '',
    execution_id TEXT REFERENCES executions(id)
);

CREATE INDEX idx_trigger_attempts_hook_slug ON trigger_attempts(hook_slug);
CREATE INDEX idx_trigger_attempts_attempted_at ON trigger_attempts(attempted_at);
CREATE INDEX idx_trigger_attempts_hook_attempted ON trigger_attempts(hook_slug, attempted_at DESC);
CREATE INDEX idx_trigger_attempts_status ON trigger_attempts(status);
CREATE INDEX idx_trigger_attempts_execution_id ON trigger_attempts(execution_id);
