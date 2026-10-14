-- The persisted shard manager state changed shape
-- Any state written by an earlier version is dropped; it is rebuilt as executors register.
DELETE FROM shard_manager_state;