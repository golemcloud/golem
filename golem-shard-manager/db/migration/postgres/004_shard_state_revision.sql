-- `revision` is a storage-level fencing token for compare-and-swap writes, unrelated to the
-- ShardLeaseRevision inside `state`; never derive one from the other. Revision 0 means "no state
-- stored", so a stored row always carries >= 1 and the column has no default.
DROP TABLE shard_manager_state;

CREATE TABLE shard_manager_state
(
    id       INTEGER PRIMARY KEY,
    state    BYTEA   NOT NULL,
    revision BIGINT  NOT NULL
);

-- A mirror of the state blob for inspection with plain SQL; rewritten wholesale on every write.
-- Clearing shard_manager_state by hand means clearing these two as well.
CREATE TABLE executor_leases
(
    executor_id UUID      NOT NULL,
    ip          BYTEA     NOT NULL,
    port        INTEGER   NOT NULL,
    granted_at  TIMESTAMP NOT NULL,
    expires_at  TIMESTAMP NOT NULL,
    pod_name    TEXT,

    CONSTRAINT executor_leases_pk
        PRIMARY KEY (executor_id)
);

CREATE TABLE shard_assignments
(
    shard_id    INTEGER NOT NULL,
    executor_id UUID    NOT NULL,
    epoch       BIGINT  NOT NULL,

    CONSTRAINT shard_assignments_pk
        PRIMARY KEY (shard_id),
    CONSTRAINT shard_assignments_executor_fk
        FOREIGN KEY (executor_id) REFERENCES executor_leases
);
