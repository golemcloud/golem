ALTER TABLE oauth2_tokens
DROP CONSTRAINT oauth2_tokens_token_id_fkey
;

ALTER TABLE oauth2_tokens
RENAME COLUMN token_id TO token_id_old
;

ALTER TABLE tokens
DROP CONSTRAINT tokens_pkey
;

ALTER TABLE tokens
RENAME COLUMN id TO secret
;

ALTER TABLE tokens
ADD CONSTRAINT tokens_secret_unique UNIQUE(secret)
;

ALTER TABLE tokens
ADD COLUMN id UUID NOT NULL DEFAULT gen_random_uuid()
;

ALTER TABLE tokens
ALTER COLUMN id DROP DEFAULT
;

ALTER TABLE tokens
ADD PRIMARY KEY (id)
;

ALTER TABLE oauth2_tokens
ADD COLUMN token_id UUID NULL REFERENCES tokens(id)
;

update oauth2_tokens ot
set
  token_id = (select id from tokens t where t.secret = ot.token_id_old)
;

ALTER TABLE oauth2_tokens
DROP COLUMN token_id_old
;

ALTER TABLE oauth2_tokens
ALTER COLUMN token_id SET NOT NULL
;
