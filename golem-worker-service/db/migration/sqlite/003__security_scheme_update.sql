CREATE TABLE security_schemes_new (
  namespace                TEXT NOT NULL,
  security_scheme_id       TEXT NOT NULL,
  provider_type            TEXT NOT NULL,
  client_id                TEXT NOT NULL,
  client_secret            TEXT NOT NULL,
  redirect_url             TEXT NOT NULL,
  scopes                   TEXT NOT NULL,
  security_scheme_metadata BLOB NOT NULL,
  PRIMARY KEY (namespace, security_scheme_id),
  UNIQUE (redirect_url)
);

INSERT INTO security_schemes_new
SELECT * FROM security_schemes;

DROP TABLE security_schemes;

ALTER TABLE security_schemes_new RENAME TO security_schemes;