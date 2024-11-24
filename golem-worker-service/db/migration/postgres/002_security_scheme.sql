CREATE TABLE security_schemes
(
    namespace                text NOT NULL,
    security_scheme_id       text NOT NULL,
    provider_type            text NOT NULL,
    client_id                text NOT NULL,
    client_secret            text NOT NULL,
    redirect_url             text NOT NULL,
    scopes                   text NOT NULL,
    security_scheme_metadata bytea NOT NULL,
    PRIMARY KEY (namespace, security_scheme_id)
);
