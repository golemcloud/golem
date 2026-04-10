-- Clean reset: provision config is now stored in component_revisions.metadata.
-- All deployment and component data is cleared as part of the reset release.

TRUNCATE TABLE deployment_compiled_http_api_definition_routes CASCADE;
TRUNCATE TABLE deployment_domain_http_api_definitions CASCADE;
TRUNCATE TABLE deployment_registered_agent_types CASCADE;
TRUNCATE TABLE deployment_http_api_deployment_revisions CASCADE;
TRUNCATE TABLE deployment_http_api_definition_revisions CASCADE;
TRUNCATE TABLE deployment_component_revisions CASCADE;
TRUNCATE TABLE current_deployment_revisions CASCADE;
TRUNCATE TABLE current_deployments CASCADE;
TRUNCATE TABLE deployment_revisions CASCADE;
TRUNCATE TABLE component_plugin_installations CASCADE;
TRUNCATE TABLE component_files CASCADE;
TRUNCATE TABLE original_component_files CASCADE;
TRUNCATE TABLE component_revisions CASCADE;
TRUNCATE TABLE components CASCADE;

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
