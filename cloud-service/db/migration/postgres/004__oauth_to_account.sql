ALTER TABLE oauth2_tokens
ADD COLUMN account_id VARCHAR(100) NULL REFERENCES accounts(id)
;

update oauth2_tokens ot
set account_id = (select t.account_id from tokens t where t.id = ot.token_id)
;

ALTER TABLE oauth2_tokens
ALTER COLUMN account_id SET NOT NULL
;

ALTER TABLE oauth2_tokens
ALTER COLUMN token_id DROP NOT NULL
;

CREATE INDEX oauth2_tokens_token_id_idx
ON oauth2_tokens(token_id)
;