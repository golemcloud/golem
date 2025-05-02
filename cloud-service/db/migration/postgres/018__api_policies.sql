ALTER TABLE project_policies
    ADD COLUMN upsert_api_deployment BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN view_api_deployment BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN delete_api_deployment BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN upsert_api_domain BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN view_api_domain BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN delete_api_domain BOOLEAN NOT NULL DEFAULT false;
