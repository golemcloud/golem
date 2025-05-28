ALTER TABLE project_policies ADD COLUMN view_api_definition BOOLEAN;
ALTER TABLE project_policies ADD COLUMN create_api_definition BOOLEAN;
ALTER TABLE project_policies ADD COLUMN update_api_definition BOOLEAN;
ALTER TABLE project_policies ADD COLUMN delete_api_definition BOOLEAN;

UPDATE project_policies SET view_api_definition = false where view_api_definition IS NULL;
UPDATE project_policies SET create_api_definition = false where create_api_definition IS NULL;
UPDATE project_policies SET update_api_definition = false where update_api_definition IS NULL;
UPDATE project_policies SET delete_api_definition = false where delete_api_definition IS NULL;

ALTER TABLE project_policies ALTER COLUMN view_api_definition SET NOT NULL;
ALTER TABLE project_policies ALTER COLUMN create_api_definition SET NOT NULL;
ALTER TABLE project_policies ALTER COLUMN update_api_definition SET NOT NULL;
ALTER TABLE project_policies ALTER COLUMN delete_api_definition SET NOT NULL;