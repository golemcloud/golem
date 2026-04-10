-- Clean reset: provision config is now stored in component_revisions.metadata.
-- All deployment and component data is cleared as part of the reset release.
-- Deletes are ordered children-before-parents to satisfy FK constraints.

DELETE FROM deployment_compiled_http_api_definition_routes;
DELETE FROM deployment_domain_http_api_definitions;
DELETE FROM deployment_registered_agent_types;
DELETE FROM deployment_http_api_deployment_revisions;
DELETE FROM deployment_http_api_definition_revisions;
DELETE FROM deployment_component_revisions;
DELETE FROM current_deployment_revisions;
DELETE FROM current_deployments;
DELETE FROM deployment_revisions;
DELETE FROM component_plugin_installations;
DELETE FROM component_files;
DELETE FROM original_component_files;
DELETE FROM component_revisions;
DELETE FROM components;

-- Drop tables whose data is now stored in metadata
DROP TABLE component_plugin_installations;
DROP TABLE component_files;
DROP TABLE original_component_files;

-- Drop columns moved into metadata
ALTER TABLE component_revisions
    DROP COLUMN env;

ALTER TABLE component_revisions
    DROP COLUMN config_vars;

ALTER TABLE component_revisions
    DROP COLUMN agent_config;
