-- Budget that a Capacity resource still owes after its limit shrank below what its leases used
-- or hold. Lease returns pay it before they add to `remaining`.
ALTER TABLE quota_resources
    ADD COLUMN deficit NUMERIC NOT NULL DEFAULT 0;
