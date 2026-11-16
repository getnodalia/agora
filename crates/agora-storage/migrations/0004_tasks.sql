-- Tasks are intentionally server-readable workspace data, not MLS messages.
CREATE TABLE tasks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id INTEGER NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('todo', 'in_progress', 'done')),
    priority TEXT NOT NULL CHECK (priority IN ('low', 'normal', 'high')),
    assignee_id INTEGER REFERENCES users(id) ON DELETE SET NULL,
    due_at INTEGER,
    created_by INTEGER NOT NULL REFERENCES users(id),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    revision INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX tasks_workspace ON tasks(workspace_id, id);
