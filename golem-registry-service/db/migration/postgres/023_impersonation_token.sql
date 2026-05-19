ALTER TABLE tokens ADD COLUMN impersonated_by UUID NULL;

CREATE INDEX tokens_impersonated_by_idx ON tokens (impersonated_by);
