-- Budget that a Capacity or Concurrency resource still owes after its limit shrank below what
-- was consumed or is leased. Lease returns pay it before they add to `remaining`.
ALTER TABLE quota_resources
    ADD COLUMN deficit NUMERIC NOT NULL DEFAULT 0;
