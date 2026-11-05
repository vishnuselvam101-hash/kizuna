-- Final review M4: remember why an account was disabled, so recovery only re-enables containment locks.
ALTER TABLE users ADD COLUMN disabled_by_action uuid;
