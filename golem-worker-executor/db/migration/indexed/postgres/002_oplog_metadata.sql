-- The shard epoch authorised to write each oplog.
--
-- One row per key in `index_storage`, carrying the ownership generation of the shard the agent
-- belongs to. An append asserts its epoch against this row inside the same transaction as the
-- insert, so an executor that has lost the shard cannot keep writing to an oplog whose owner has
-- moved on. A missing row fences too: the row is written before the first entry, and removed
-- before the entries are.
CREATE TABLE oplog_metadata (
    namespace TEXT NOT NULL,
    key TEXT NOT NULL,
    epoch BIGINT NOT NULL,
    PRIMARY KEY (namespace, key)
);
