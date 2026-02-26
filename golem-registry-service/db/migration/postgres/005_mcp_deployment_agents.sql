-- Add agents data column to mcp_deployment_revisions table
ALTER TABLE mcp_deployment_revisions
    ADD COLUMN data BYTEA NOT NULL DEFAULT '{"agents":{}}';

-- Remove the default after adding the column
ALTER TABLE mcp_deployment_revisions
    ALTER COLUMN data DROP DEFAULT;
