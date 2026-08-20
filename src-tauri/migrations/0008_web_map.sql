-- Which web map an instance is meant to have.
--
-- Set when the create dialog's "Web map" box is ticked, and read after the
-- server install finishes so the mod is installed into a folder that exists.
-- The *port* is deliberately not stored: both map mods write a config the user
-- is free to edit, so it is read from that file rather than remembered here.
ALTER TABLE instances ADD COLUMN map_kind TEXT;
