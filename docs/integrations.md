# Integrations

An integration lets another app post notices to a channel: a Hito project, a build server, a
script. A workspace admin adds one from the channel's details (**Integrations**) or from the
global menu (**Conversation ▸ Integrations**). Ágora then shows an address and a key, once. The
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
an address reveals nothing on its own. A body is at most 64 KiB, and one integration can post 60
notices a minute.

Sending the same body twice (a retry) records it once. To post the same text again, change
something in the body, such as a timestamp field; unknown fields are ignored.

## Hito

Hito's webhooks are understood as they are: Hito signs with `X-Hito-Signature` in the same form,
and its payload's `text`, `project.name` (shown as the title) and `server_url` (the link) are
used. In Hito, add an Ágora connection to the project and paste the address and the key.

## Keeping it safe

- The key is 32 random bytes, shown as 64 hexadecimal characters once. Ágora keeps it to check
  signatures; anyone who can read the server's database can post notices.
- Removing an integration stops its address at once. Its notices stay in the channel.
- Only workspace admins add or remove integrations, and not in direct conversations. Everyone who
  can read the channel sees which integrations it has. Someone who cannot read the channel gets
  `404` for all of it.
