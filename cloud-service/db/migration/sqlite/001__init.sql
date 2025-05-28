CREATE TABLE plans
(
    plan_id              uuid    NOT NULL PRIMARY KEY,
    project_limit        integer NOT NULL,
    component_limit      integer NOT NULL,
    worker_limit         integer NOT NULL,
    storage_limit        integer NOT NULL,
    monthly_gas_limit    bigint  NOT NULL,
    monthly_upload_limit integer NOT NULL,
    created_at           timestamp without time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE accounts
(
    id      character varying(100) NOT NULL PRIMARY KEY,
    name    text                   NOT NULL,
    email   text                   NOT NULL,
    plan_id uuid                   NOT NULL,
    deleted boolean DEFAULT false  NOT NULL,
    FOREIGN KEY (plan_id) REFERENCES plans (plan_id)
);


CREATE TABLE projects
(
    project_id  uuid NOT NULL PRIMARY KEY,
    name        text NOT NULL,
    description text NOT NULL,
    created_at  timestamp without time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE account_connections
(
    account_id character varying(100) NOT NULL PRIMARY KEY,
    counter    integer                NOT NULL,
    FOREIGN KEY (account_id) REFERENCES accounts (id)
);

CREATE TABLE account_creation_attempts
(
    oauth2_provider text    NOT NULL,
    external_id     text    NOT NULL,
    name            text    NOT NULL,
    email           text    NOT NULL,
    all_emails      jsonb   NOT NULL,
    first_attempt   timestamp without time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    latest_attempt  timestamp without time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    attempts_count  integer NOT NULL,
    PRIMARY KEY (oauth2_provider, external_id)
);

CREATE TABLE account_fuel
(
    account_id character varying(100) NOT NULL PRIMARY KEY,
    consumed   bigint                 NOT NULL,
    month      integer                NOT NULL,
    year       integer                NOT NULL,
    FOREIGN KEY (account_id) REFERENCES accounts (id)
);

CREATE TABLE account_grants
(
    account_id character varying(100) NOT NULL PRIMARY KEY,
    role_id    text                   NOT NULL,
    FOREIGN KEY (account_id) REFERENCES accounts (id)
);

CREATE TABLE account_workers
(
    account_id character varying(100) NOT NULL PRIMARY KEY,
    counter    integer                NOT NULL,
    FOREIGN KEY (account_id) REFERENCES accounts (id)
);

CREATE TABLE account_components
(
    account_id character varying(100) NOT NULL PRIMARY KEY,
    counter    integer                NOT NULL,
    FOREIGN KEY (account_id) REFERENCES accounts (id)
);

CREATE TABLE account_used_storage
(
    account_id character varying(100) NOT NULL PRIMARY KEY,
    counter    bigint                 NOT NULL,
    FOREIGN KEY (account_id) REFERENCES accounts (id)
);

CREATE TABLE account_uploads
(
    account_id character varying(100) NOT NULL PRIMARY KEY,
    counter    integer                NOT NULL,
    month      integer                NOT NULL,
    year       integer                NOT NULL,
    FOREIGN KEY (account_id) REFERENCES accounts (id)
);


CREATE TABLE tokens
(
    secret     uuid                   NOT NULL,
    account_id character varying(100) NOT NULL,
    created_at timestamp without time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    expires_at timestamp without time zone NOT NULL,
    id         uuid                   NOT NULL PRIMARY KEY,
    FOREIGN KEY (account_id) REFERENCES accounts (id)
);


CREATE TABLE oauth2_tokens
(
    provider    text                   NOT NULL,
    external_id text                   NOT NULL,
    token_id    uuid,
    account_id  character varying(100) NOT NULL,
    PRIMARY KEY (provider, external_id),
    FOREIGN KEY (account_id) REFERENCES accounts (id),
    FOREIGN KEY (token_id) REFERENCES tokens (id)
);

CREATE TABLE IF NOT EXISTS oauth2_web_flow_state (
    oauth2_state TEXT PRIMARY KEY,
    metadata     BLOB NOT NULL,
    -- NULL token_id indicates pending OAuth callback linkage
    token_id     TEXT REFERENCES tokens(id),
    created_at   TIMESTAMP WITHOUT TIME ZONE DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE project_account
(
    project_id       uuid                   NOT NULL,
    owner_account_id character varying(100) NOT NULL,
    is_default       boolean,
    FOREIGN KEY (project_id) REFERENCES projects (project_id),
    FOREIGN KEY (owner_account_id) REFERENCES accounts (id)
);


CREATE TABLE project_policies
(
    project_policy_id     uuid    NOT NULL PRIMARY KEY,
    name                  text    NOT NULL,
    view_component        boolean NOT NULL,
    create_component      boolean NOT NULL,
    update_component      boolean NOT NULL,
    delete_component      boolean NOT NULL,
    view_worker           boolean NOT NULL,
    create_worker         boolean NOT NULL,
    update_worker         boolean NOT NULL,
    delete_worker         boolean NOT NULL,
    view_project_grants   boolean NOT NULL,
    create_project_grants boolean NOT NULL,
    delete_project_grants boolean NOT NULL,
    view_api_definition   boolean NOT NULL,
    create_api_definition boolean NOT NULL,
    update_api_definition boolean NOT NULL,
    delete_api_definition boolean NOT NULL
);


CREATE TABLE project_grants
(
    project_grant_id   uuid                   NOT NULL PRIMARY KEY,
    grantee_account_id character varying(100) NOT NULL,
    grantor_project_id uuid                   NOT NULL,
    project_policy_id  uuid                   NOT NULL,
    created_at         timestamp without time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    FOREIGN KEY (grantor_project_id) REFERENCES projects (project_id),
    FOREIGN KEY (grantee_account_id) REFERENCES accounts (id),
    FOREIGN KEY (project_policy_id) REFERENCES project_policies (project_policy_id)
);


CREATE INDEX IF NOT EXISTS accounts_project_id on project_account(owner_account_id);

CREATE UNIQUE INDEX only_one_default_project_per_owner
    ON project_account (owner_account_id) WHERE (is_default);
