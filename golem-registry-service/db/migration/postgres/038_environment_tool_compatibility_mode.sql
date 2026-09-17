ALTER TABLE environment_revisions
    ADD COLUMN tool_compatibility_mode TEXT NOT NULL DEFAULT 'structural-subtype';
