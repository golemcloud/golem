-- The persisted shard lease state is now written with a compare-and-swap guard, and mirrored into
-- queryable tables.
--
-- `revision` is a storage-level fencing token. It is incremented by exactly one on every
-- successful write and is deliberately UNRELATED to the domain-level ShardLeaseRevision inside
-- `state`: that one starts at 0 and is only bumped when the routing table meaningfully changes.
-- Never derive this column from it.
--
-- Revision 0 is reserved to mean "no state stored", so a stored row always carries >= 1. The
-- column therefore has NO DEFAULT: a write that forgets to bind it must fail loudly.
--
-- The table is recreated rather than altered: any row written before this migration has no
-- meaningful revision, and the state rebuilds itself from executor registrations (same reasoning
-- as 003). Recreating also avoids SQLite's requirement that ADD COLUMN supply a non-null default.
DROP TABLE shard_manager_state;

CREATE TABLE shard_manager_state
(
    id       INTEGER PRIMARY KEY,
    state    BYTEA   NOT NULL,
    revision BIGINT  NOT NULL
);

-- Local-mode mirror of the state blob, for inspection with plain SQL (`executor_leases` is the
-- counterpart of the quota system's `quota_leases`). The blob is the source of truth: both tables
-- are rewritten wholesale in the same transaction as every write the shard manager makes, and it
-- never reads them back. Not mirrored: `pending_rebalance`, `shard_epochs` (the per-shard epoch
-- high-water marks, which outlive leases) and the in-blob `ShardLeaseRevision`.
--
-- Anyone clearing the state by hand (`DELETE FROM shard_manager_state`) must clear these two
-- tables as well, or they keep describing leases that no longer exist until the next write.
--
-- The shard manager refuses to persist a state whose assignments reference an executor without a
-- lease, so the foreign key below is belt-and-braces. Postgres always enforces it; SQLite only
-- with `foreign_keys = true` (the default is `false`).
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
