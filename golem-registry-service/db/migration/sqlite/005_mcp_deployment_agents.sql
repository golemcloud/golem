-- Add agents data column to mcp_deployment_revisions table
ALTER TABLE mcp_deployment_revisions
    ADD COLUMN data BLOB NOT NULL DEFAULT X'7B22616765767473223A7B7D7D';
