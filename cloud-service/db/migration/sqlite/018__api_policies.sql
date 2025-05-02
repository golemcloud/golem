ALTER TABLE project_policies ADD COLUMN upsert_api_deployment BOOLEAN DEFAULT false;
ALTER TABLE project_policies ADD COLUMN view_api_deployment BOOLEAN DEFAULT false;
ALTER TABLE project_policies ADD COLUMN delete_api_deployment BOOLEAN DEFAULT false;
ALTER TABLE project_policies ADD COLUMN upsert_api_domain BOOLEAN DEFAULT false;
ALTER TABLE project_policies ADD COLUMN view_api_domain BOOLEAN DEFAULT false;
ALTER TABLE project_policies ADD COLUMN delete_api_domain BOOLEAN DEFAULT false;
