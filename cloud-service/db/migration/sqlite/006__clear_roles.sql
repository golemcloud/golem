-- Clear everything except Admin and MarketingAdmin
DELETE FROM account_grants
WHERE role_id NOT IN ('Admin', 'MarketingAdmin')
