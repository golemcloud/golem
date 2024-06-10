CREATE TABLE api_definitions
(
    id                   uuid      NOT NULL,
    version              bigint    NOT NULL,
    draft                boolean   NOT NULL default true,
    routes               jsonb     NOT NULL,
    created_at           timestamp NOT NULL DEFAULT CURRENT_TIMESTAMP,
    deployment_host      text      NOT NULL,
    deployment_subdomain text,
    PRIMARY KEY (id, version)
);


CREATE TABLE api_deployments
(
    host       text      NOT NULL,
    subdomain  text,
    created_at timestamp NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (id, version)
);
