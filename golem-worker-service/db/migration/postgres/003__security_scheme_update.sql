ALTER TABLE security_schemes
    ADD CONSTRAINT unique_redirect_scopes UNIQUE (redirect_url);