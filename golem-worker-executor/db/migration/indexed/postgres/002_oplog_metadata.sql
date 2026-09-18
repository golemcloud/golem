-- The shard epoch authorised to write each oplog, and the writer holding it.
--
-- One row per key in `index_storage`, carrying the ownership generation of the shard the agent
-- belongs to. An append asserts its epoch against this row inside the same transaction as the
-- insert, so an executor that has lost the shard cannot keep writing to an oplog whose owner has
-- moved on. A missing row fences too: the row is written before the first entry, and removed
-- before the entries are.
--
-- `owner` is the writing process, not the executor's lease identity, which is regenerated whenever
-- the shard manager answers `LeaseNotFound`. The epoch alone cannot separate two writers holding
-- the same number, which a shard manager that lost its state hands out when it mints from zero
-- again: the process that recorded the row is the one allowed to go on writing at that epoch, and
-- any other writer is refused and reports it, so the manager mints above and the takeover happens
-- at a generation nobody shares.
CREATE TABLE oplog_metadata (
    namespace TEXT NOT NULL,
    key TEXT NOT NULL,
    epoch BIGINT NOT NULL,
    owner TEXT NOT NULL,
    PRIMARY KEY (namespace, key)
);
