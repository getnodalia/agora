# GitHub webhook inbox (server API)

This experimental server-side inbox receives a limited set of GitHub repository webhook events.
It is separate from conversations: the event type, action, issue or pull request number, receipt
time, delivery identifier and a hash of the payload are server-readable metadata. Payloads,
titles, comments and secrets are not saved. No integration can read encrypted Ágora messages.
The desktop app displays the latest activity but cannot configure the inbox yet.

The server operator can configure one repository and one workspace using all three environment
variables: `AGORA_GITHUB_WORKSPACE_ID`, `AGORA_GITHUB_REPOSITORY_ID` (GitHub's numeric repository
ID) and `AGORA_GITHUB_WEBHOOK_SECRET` (32–256 bytes). Keep the unique random secret out of source
control and logs; environment variables are accessible to privileged processes on the host.
Remove the variables and restart to disable the receiver. The workspace must already exist.
In GitHub configure a JSON webhook over HTTPS at `/api/v1/integrations/github/webhook` with the same
secret and select the desired events there. Limit its repository access and choose only events
whose metadata the workspace may see. No outgoing calls or actions on GitHub are made.

The receiver verifies GitHub's HMAC-SHA256 over the raw body before parsing it and checks the
signed repository ID. Requests are bounded to 1 MiB and rate-limited per client IP, using the
server's trusted-proxy settings. GitHub signs the body, not the event or delivery headers; do not
use those headers as proof of authorization. Repeated delivery IDs or identical body hashes are
ignored even after display records are pruned; replay markers remain until the workspace is
deleted and consume storage over time. The most recent 5,000 display records are retained per
workspace. Identical legitimate payloads may be collapsed. Events larger than the
limit are rejected, and GitHub may require manual redelivery after a failure.

Workspace members can read the latest 100 records using authenticated
`GET /api/v1/workspaces/{id}/integrations/github/events`; non-members get 404. This initial API
does not support multiple installations, administrator configuration or revocation without a
restart, filtering by channel, fine-grained event permissions, pagination, or automatic refresh.
Do not treat it as a complete integrations platform.
