-- Workspace icons are public to workspace members and are not end-to-end encrypted.
ALTER TABLE workspaces ADD COLUMN icon_png BLOB;
