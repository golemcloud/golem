DELETE FROM deployment_http_api_deployment_revisions;
DELETE FROM http_api_deployment_revisions;
DELETE FROM http_api_deployments;

ALTER TABLE http_api_deployment_revisions RENAME COLUMN http_api_definitions TO agent_types;

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
