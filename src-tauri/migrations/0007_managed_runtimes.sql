-- JDKs this app downloads and owns, one per feature version and shared by every
-- instance that needs it. Keyed by version rather than by instance: a second
-- server needing Java 25 reuses the first one's download.
CREATE TABLE managed_runtimes (
    feature_version INTEGER PRIMARY KEY,
    release_name    TEXT    NOT NULL,
    vendor          TEXT    NOT NULL,
    -- Absolute path to the java binary inside the install.
    java_path       TEXT    NOT NULL,
    installed_at    TEXT    NOT NULL,
    size_bytes      INTEGER NOT NULL DEFAULT 0
);
