CREATE TABLE api_definitions
(
    namespace  text    NOT NULL,
    id         text    NOT NULL,
    version    text    NOT NULL,
    draft      boolean NOT NULL default true,
    data       blob    NOT NULL,
    created_at timestamp without time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    PRIMARY KEY (namespace, id, version)
);


CREATE TABLE api_deployments
(
    namespace          text NOT NULL,
    site               text NOT NULL,
    host               text NOT NULL,
    subdomain          text,
    definition_id      text NOT NULL,
    definition_version text NOT NULL,
    created_at         timestamp without time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    PRIMARY KEY (namespace, site, definition_id, definition_version)
);

CREATE TABLE api_certificates
(
    namespace   text NOT NULL,
    id          UUID NOT NULL,
    domain_name text NOT NULL,
    external_id text NOT NULL,
    created_at  timestamp without time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    PRIMARY KEY (namespace, id)
);

CREATE TABLE api_domains
(
    namespace    text NOT NULL,
    domain_name  text NOT NULL,
    name_servers text NOT NULL,
    created_at   timestamp without time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    PRIMARY KEY (namespace, domain_name)
);

