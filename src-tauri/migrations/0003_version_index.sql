-- Release chronology from Mojang's version manifest.
--
-- Version ordering is never derived from parsing the version string: the two
-- eras (1.21.11 and 26.2) sort correctly only because Mojang says which came
-- first. This table caches that ordering so sorting still works offline.
CREATE TABLE mc_version_index (
    id           TEXT PRIMARY KEY,
    -- RFC3339 release timestamp as published by Mojang.
    release_time TEXT    NOT NULL,
    -- release | snapshot | old_beta | old_alpha
    kind         TEXT    NOT NULL,
    -- Position in the manifest, 0 = newest. Ties break on this.
    position     INTEGER NOT NULL
);

CREATE INDEX idx_mc_version_index_release ON mc_version_index(release_time DESC);
