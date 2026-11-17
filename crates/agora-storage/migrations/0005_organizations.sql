-- Existing workspaces remain independent and retain their members and permissions.
CREATE TABLE organizations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    owner_id INTEGER NOT NULL REFERENCES users(id),
    created_at INTEGER NOT NULL
);
CREATE TABLE organization_members (
    organization_id INTEGER NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK (role IN ('owner', 'admin', 'member', 'guest')),
    joined_at INTEGER NOT NULL,
    PRIMARY KEY (organization_id, user_id)
);
CREATE UNIQUE INDEX organization_one_owner ON organization_members(organization_id) WHERE role = 'owner';
CREATE INDEX organization_members_user ON organization_members(user_id);
ALTER TABLE workspaces ADD COLUMN organization_id INTEGER REFERENCES organizations(id);
CREATE INDEX workspaces_organization ON workspaces(organization_id);
