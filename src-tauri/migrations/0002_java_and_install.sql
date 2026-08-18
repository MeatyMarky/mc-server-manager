-- Java runtime cache gains the fields detection needs to stay cheap, and
-- instances remember how their server was installed.

-- Re-detection compares the binary's mtime and size against what was recorded;
-- when both match, `java -version` does not need to run again.
ALTER TABLE java_runtimes ADD COLUMN mtime INTEGER;
ALTER TABLE java_runtimes ADD COLUMN size_bytes INTEGER;
ALTER TABLE java_runtimes ADD COLUMN full_version TEXT;
ALTER TABLE java_runtimes ADD COLUMN error TEXT;

-- Which artifact produced the current install, so a repair or a version change
-- knows what it is replacing.
ALTER TABLE instances ADD COLUMN installed_artifact_url TEXT;
ALTER TABLE instances ADD COLUMN installed_at TEXT;
