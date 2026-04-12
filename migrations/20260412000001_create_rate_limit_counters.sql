CREATE TABLE rate_limit_counters (
    hook_slug    TEXT NOT NULL,
    window_start TEXT NOT NULL,
    count        INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (hook_slug, window_start)
);
