CREATE EXTENSION IF NOT EXISTS pgcrypto;

CREATE TABLE IF NOT EXISTS plans (
    plan_id UUID,
    project_limit INTEGER NOT NULL,
    component_limit INTEGER NOT NULL,
    instance_limit INTEGER NOT NULL,
    storage_limit INTEGER NOT NULL,
    monthly_gas_limit BIGINT NOT NULL,
    monthly_upload_limit INTEGER NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (plan_id)
);

CREATE TABLE IF NOT EXISTS accounts(
    id VARCHAR(100) NOT NULL,
    name TEXT NOT NULL,
    email TEXT NOT NULL,
    plan_id UUID NOT NULL REFERENCES plans(plan_id),
    deleted BOOLEAN NOT NULL DEFAULT false,
    PRIMARY KEY (id)
);

CREATE TABLE IF NOT EXISTS projects (
    project_id UUID,
    name TEXT NOT NULL,
    description TEXT NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (project_id)
);

-- https://www.notion.so/Golem-Front-end-1aa11fd41dcf4c878b465c15ca815d61?d=00f52b947a4a405cba0a50bc5ca63ff4&pvs=4#50f5e0f4b9054785b13f62759fc78d66
CREATE TABLE IF NOT EXISTS project_account (
  project_id UUID NOT NULL REFERENCES projects(project_id),
  owner_account_id VARCHAR(100) NOT NULL REFERENCES accounts(id),
  is_default BOOLEAN,
  PRIMARY KEY (project_id)
);

CREATE INDEX IF NOT EXISTS accounts_project_id on project_account(owner_account_id);

CREATE UNIQUE INDEX only_one_default_project_per_owner
  ON project_account (owner_account_id)
  WHERE (is_default);

ALTER TABLE project_account ADD CONSTRAINT only_one_default_project
  UNIQUE(project_id, owner_account_id);

CREATE TABLE IF NOT EXISTS components (
    component_id UUID NOT NULL,
    project_id UUID NOT NULL REFERENCES projects(project_id),
    name TEXT NOT NULL,
    size INTEGER NOT NULL,
    version INTEGER NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    user_component TEXT NOT NULL,
    protected_component TEXT NOT NULL,
    protector_version INTEGER,
    metadata JSONB NOT NULL,
    PRIMARY KEY (component_id, version)
);


CREATE TABLE IF NOT EXISTS project_policies (
  project_policy_id UUID NOT NULL PRIMARY KEY,
  name TEXT NOT NULL,
  view_component BOOLEAN NOT NULL,
  create_component BOOLEAN NOT NULL,
  update_component BOOLEAN NOT NULL,
  delete_component BOOLEAN NOT NULL,
  view_instance BOOLEAN NOT NULL,
  create_instance BOOLEAN NOT NULL,
  update_instance BOOLEAN NOT NULL,
  delete_instance BOOLEAN NOT NULL,
  view_project_grants BOOLEAN NOT NULL,
  create_project_grants BOOLEAN NOT NULL,
  delete_project_grants BOOLEAN NOT NULL
);

CREATE TABLE IF NOT EXISTS tokens (
  id UUID NOT NULL,
  account_id VARCHAR(100) NOT NULL REFERENCES accounts(id),
  created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
  expires_at TIMESTAMP NOT NULL,
  PRIMARY KEY (id)
);

CREATE TABLE IF NOT EXISTS project_grants (
  project_grant_id UUID PRIMARY KEY,
  grantee_account_id  VARCHAR(100) NOT NULL REFERENCES accounts(id),
  grantor_project_id UUID NOT NULL REFERENCES projects(project_id),
  project_policy_id UUID NOT NULL REFERENCES project_policies(project_policy_id),
  created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS tokens_account_id on tokens(account_id);

CREATE TABLE IF NOT EXISTS account_grants (
  account_id VARCHAR(100) NOT NULL REFERENCES accounts(id),
  role_id TEXT NOT NULL,
  PRIMARY KEY (account_id, role_id)
);

CREATE TABLE IF NOT EXISTS oauth2_tokens (
    provider TEXT NOT NULL,
    external_id TEXT NOT NULL,
    token_id UUID NOT NULL REFERENCES tokens(id),
    PRIMARY KEY (provider, external_id)
);

CREATE TABLE IF NOT EXISTS account_instances (
    account_id VARCHAR(100) NOT NULL REFERENCES accounts(id),
    counter INTEGER NOT NULL,
    PRIMARY KEY (account_id)
);

CREATE TABLE IF NOT EXISTS account_connections (
    account_id VARCHAR(100) NOT NULL REFERENCES accounts(id),
    counter INTEGER NOT NULL,
    PRIMARY KEY (account_id)
);

CREATE TABLE IF NOT EXISTS account_uploads (
    account_id VARCHAR(100) NOT NULL REFERENCES accounts(id),
    counter INTEGER NOT NULL,
    month INTEGER NOT NULL,
    year INTEGER NOT NULL,
    PRIMARY KEY (account_id)
);

CREATE TABLE IF NOT EXISTS account_fuel (
    account_id VARCHAR(100) NOT NULL REFERENCES accounts(id),
    consumed BIGINT NOT NULL,
    month INTEGER NOT NULL,
    year INTEGER NOT NULL,
    PRIMARY KEY (account_id)
);

CREATE OR REPLACE FUNCTION get_current_date()
RETURNS DATE AS
$$
BEGIN
    RETURN CURRENT_DATE;
END;
$$ LANGUAGE plpgsql;
