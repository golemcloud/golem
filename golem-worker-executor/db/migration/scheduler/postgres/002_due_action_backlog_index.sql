CREATE INDEX scheduled_actions_shard_due_at_idx
    ON scheduled_actions (shard_id, due_at_ms);
