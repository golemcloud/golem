ALTER TABLE plans RENAME COLUMN template_limit TO component_limit;

ALTER TABLE project_policies RENAME COLUMN view_template TO view_component;
ALTER TABLE project_policies RENAME COLUMN create_template TO create_component;
ALTER TABLE project_policies RENAME COLUMN update_template TO update_component;
ALTER TABLE project_policies RENAME COLUMN delete_template TO delete_component;

ALTER TABLE templates RENAME TO components;
ALTER TABLE components RENAME COLUMN template_id TO component_id;
ALTER TABLE components RENAME COLUMN user_template TO user_component;
ALTER TABLE components RENAME COLUMN protected_template TO protected_component;