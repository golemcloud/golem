DROP INDEX plugins_name_version_unique;
CREATE UNIQUE INDEX plugins_account_id_name_version_unique ON plugins (account_id, name, version) WHERE (deleted IS FALSE);
