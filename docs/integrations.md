# Integrations

An integration lets another app post notices to a channel: a Hito project, a build server, a
script. A workspace admin adds one from the channel's details (**Integrations**) or from the
global menu (**Conversation ▸ Integrations**). The window has two tabs: **Connected**, and **Plugins**, a gallery of what can be connected (Hito, GitHub, a build server, monitoring, a script, any other app that sends a signed web request). Hito connects in one click; the others ask for a name. Ágora then shows an address and a key, once. The
other app sends each notice to that address, signed with the key.

Notices are **not end-to-end encrypted**. The server reads and stores them so that every member
sees them, including those who join later. They are drawn apart from messages, with the app's
name and a note saying so. Do not send anything through an integration that the server must not
read.

## Sending a notice

```
POST /api/v1/hooks/{id}
Content-Type: application/json
X-Agora-Signature: sha256=<hex HMAC-SHA256 of the raw body, keyed with the key as text>

{"text": "Build 41 passed", "title": "CI", "url": "https://ci.example/41"}
```

| Field | |
|---|---|
| `text` | Required. At most 2,000 characters; control characters other than new lines are dropped. |
| `title` | Optional. At most 200 characters. |
| `url` | Optional. An `http` or `https` link of at most 500 characters; anything else is dropped. |

The answer is `202 Accepted`. An unknown integration and a wrong signature both answer `401`, so
an address reveals nothing on its own. A body is at most 1 MiB, and one integration can post 60
notices a minute.

Sending the same body twice (a retry) records it once. To post the same text again, change
something in the body, such as a timestamp field; unknown fields are ignored.

## Hito

Hito's webhooks are understood as they are: Hito signs with `X-Hito-Signature` in the same form,
and its payload's `text`, `project.name` (shown as the title) and `server_url` (the link) are
used. In Hito, add an Ágora connection to the project and paste the address and the key.

## GitHub

Choose **GitHub** in the Plugins tab and name the connection after the repository. Ágora shows an
address and a key; on GitHub, open the repository's **Settings ▸ Webhooks ▸ Add webhook**, paste
them as the Payload URL and the Secret, choose **Send me everything** and press **Add webhook**.
GitHub's first message (a ping) appears in the channel as "GitHub is connected". The address has
to be reachable from GitHub, so a server on a home network needs a public https address.

GitHub's own headers are understood: it signs with `X-Hub-Signature-256` in the same form, names
the event in `X-GitHub-Event`, and either content type works (JSON, or GitHub's default form
encoding). Ágora turns these events into one short notice each and silently accepts the rest, so
GitHub shows a green tick for every delivery:

| Event | Notice |
|---|---|
| `push` | Who pushed how many commits to which branch, with the first three messages and a compare link. A deleted branch is reported; tags and empty pushes are not. |
| `pull_request` | Opened, reopened, ready for review, merged or closed. |
| `issues` | Opened, reopened or closed. |
| `issue_comment`, `pull_request_review_comment` | A new comment, with its first line. |
| `pull_request_review` | Approved, changes requested or reviewed. |
| `release` | A published release. |
| `workflow_run` | Only runs that failed or timed out; a green run is not news. |

Each GitHub connection has its own key, so one leaked key cannot post to another channel. Notices
carry titles, commit messages and comment lines from the repository, readable by the server and
every member of the channel: connect a private repository only to a channel with the right people.

The separate operator-configured inbox described in [github-webhook.md](github-webhook.md) is
unchanged.

## Keeping it safe

- The key is 32 random bytes, shown as 64 hexadecimal characters once. Ágora keeps it to check
  signatures; anyone who can read the server's database can post notices.
- Removing an integration stops its address at once. Its notices stay in the channel.
- Only workspace admins add or remove integrations, and not in direct conversations. Everyone who
  can read the channel sees which integrations it has. Someone who cannot read the channel gets
  `404` for all of it.
