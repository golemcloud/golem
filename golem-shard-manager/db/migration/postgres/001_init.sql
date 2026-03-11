CREATE TABLE IF NOT EXISTS shard_manager_state
(
    id    INTEGER PRIMARY KEY,
    state BYTEA NOT NULL
);
