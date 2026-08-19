-- A 32-bit JVM cannot hold a server's heap: `-Xmx8192M` makes it refuse to
-- start with "Invalid maximum heap size". Bitness now decides whether a runtime
-- is offered at all, so it is recorded rather than inferred from a folder name.
ALTER TABLE java_runtimes ADD COLUMN bits INTEGER;

-- Existing rows carry an arch guess from the same "64-Bit" marker. Backfilling
-- from it keeps the old rows usable; anything else stays NULL and is re-probed
-- on the next scan, because an unknown width must not be treated as 64-bit.
UPDATE java_runtimes SET bits = 64 WHERE arch = 'x64';
UPDATE java_runtimes SET bits = 32 WHERE arch = 'x86';
