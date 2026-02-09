DELETE FROM deployment_registered_agent_types;
DELETE FROM deployment_compiled_http_api_definition_routes;
DELETE FROM deployment_domain_http_api_definitions;
DELETE FROM deployment_http_api_deployment_revisions;
DELETE FROM deployment_http_api_definition_revisions;
DELETE FROM deployment_component_revisions;
DELETE FROM current_deployments;
DELETE FROM current_deployment_revisions;
DELETE FROM deployment_revisions;
DELETE FROM http_api_deployment_revisions;
DELETE FROM http_api_deployments;
DELETE FROM http_api_definition_revisions;
DELETE FROM http_api_definitions;
DELETE FROM original_component_files;
DELETE FROM component_files;
DELETE FROM component_plugin_installations;
DELETE FROM component_revisions;
DELETE FROM components;

DROP TABLE http_api_deployment_revisions;

CREATE TABLE http_api_deployment_revisions
(
    http_api_deployment_id UUID      NOT NULL,
    revision_id            BIGINT    NOT NULL,

    hash                   BYTEA     NOT NULL,

    created_at             TIMESTAMP NOT NULL,
    created_by             UUID      NOT NULL,
    deleted                BOOLEAN   NOT NULL,

    data                   BYTEA      NOT NULL,

    CONSTRAINT http_api_deployment_revisions_pk
        PRIMARY KEY (http_api_deployment_id, revision_id),
    CONSTRAINT http_api_deployment_revisions_deployments_fk
        FOREIGN KEY (http_api_deployment_id)
            REFERENCES http_api_deployments
);

CREATE INDEX http_api_deployment_revisions_latest_revision_by_id_idx
    ON http_api_deployment_revisions (http_api_deployment_id, revision_id DESC);

DROP TABLE deployment_compiled_http_api_definition_routes;
DROP TABLE deployment_domain_http_api_definitions;
DROP TABLE deployment_http_api_definition_revisions;

CREATE TABLE deployment_compiled_routes
(
    environment_id         UUID    NOT NULL,
    deployment_revision_id BIGINT  NOT NULL,
    domain                 TEXT   NOT NULL,
    route_id                     INTEGER NOT NULL,

    security_scheme        TEXT,             -- nullable if no security
    compiled_route         BYTEA   NOT NULL, -- full compiled route as blob

    CONSTRAINT deployment_routes_pk
        PRIMARY KEY (environment_id, deployment_revision_id, domain, route_id),

    CONSTRAINT deployment_routes_environments_fk
        FOREIGN KEY (environment_id)
            REFERENCES environments (environment_id),

    CONSTRAINT deployment_routes_deployment_revisions_fk
        FOREIGN KEY (environment_id, deployment_revision_id)
            REFERENCES deployment_revisions (environment_id, revision_id)
);

CREATE INDEX deployment_compiled_routes_domain_idx
    ON deployment_compiled_routes (domain);

ALTER TABLE deployment_registered_agent_types
    ADD webhook_prefix_authority_and_path TEXT;
