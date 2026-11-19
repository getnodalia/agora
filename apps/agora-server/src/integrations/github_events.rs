//! Turns a GitHub webhook delivery into a short, readable notice for a channel.
//!
//! The payload is hostile input: every field is optional and read defensively, and the result is
//! cleaned and cut to size by the caller. Events nobody reads in a chat (stars, forks, labels…)
//! are ignored rather than posted.

use serde_json::Value;

/// What a channel shows for one delivery.
#[derive(Debug, PartialEq, Eq)]
pub struct Summary {
    pub title: String,
    pub text: String,
    pub url: Option<String>,
}

/// The JSON of a delivery sent as `application/x-www-form-urlencoded` (GitHub's default content
/// type): the body is `payload=<percent-encoded JSON>`. The signature covers that whole body.
pub fn form_payload(body: &[u8]) -> Option<Vec<u8>> {
    let pair = body.split(|b| *b == b'&').find_map(|pair| {
        let rest = pair.strip_prefix(b"payload=")?;
        Some(rest)
    })?;
    let mut out = Vec::with_capacity(pair.len());
    let mut bytes = pair.iter().copied();
    while let Some(b) = bytes.next() {
        match b {
            b'+' => out.push(b' '),
            b'%' => {
                let high = char::from(bytes.next()?).to_digit(16)?;
                let low = char::from(bytes.next()?).to_digit(16)?;
                out.push(u8::try_from(high * 16 + low).ok()?);
            }
            other => out.push(other),
        }
    }
    Some(out)
}

fn str_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut at = value;
    for key in path {
        at = at.get(*key)?;
    }
    at.as_str().filter(|s| !s.trim().is_empty())
}

fn number_at(value: &Value, path: &[&str]) -> Option<u64> {
    let mut at = value;
    for key in path {
        at = at.get(*key)?;
    }
    at.as_u64()
}

/// The first line of a message, so a commit body does not flood the channel.
fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("").trim()
}

fn who(payload: &Value) -> &str {
    str_at(payload, &["sender", "login"]).unwrap_or("Someone")
}

fn repo(payload: &Value) -> &str {
    str_at(payload, &["repository", "full_name"]).unwrap_or("a repository")
}

fn branch(reference: &str) -> &str {
    reference.strip_prefix("refs/heads/").unwrap_or(reference)
}

/// A summary of one delivery, or `None` for an event or action that is not worth a notice.
pub fn summarize(event: &str, payload: &Value) -> Option<Summary> {
    let repo = repo(payload);
    let action = str_at(payload, &["action"]).unwrap_or("");
    match event {
        "ping" => Some(Summary {
            title: repo.to_owned(),
            text: "GitHub is connected. Pushes, pull requests, issues and releases of this \
                   repository will show up here."
                .to_owned(),
            url: str_at(payload, &["repository", "html_url"]).map(str::to_owned),
        }),
        "push" => {
            let reference = str_at(payload, &["ref"])?;
            // Tags have their own `create` event; a deleted branch has no commits to show.
            let name = reference.strip_prefix("refs/heads/")?;
            if payload.get("deleted").and_then(Value::as_bool) == Some(true) {
                return Some(Summary {
                    title: repo.to_owned(),
                    text: format!("{} deleted the branch {name}", who(payload)),
                    url: None,
                });
            }
            let commits = payload.get("commits").and_then(Value::as_array)?;
            if commits.is_empty() {
                return None;
            }
            let total = number_at(payload, &["size"])
                .or_else(|| number_at(payload, &["distinct_size"]))
                .unwrap_or(commits.len() as u64)
                .max(commits.len() as u64);
            let mut text = format!(
                "{} pushed {} to {}",
                str_at(payload, &["pusher", "name"]).unwrap_or_else(|| who(payload)),
                if total == 1 {
                    "1 commit".to_owned()
                } else {
                    format!("{total} commits")
                },
                branch(reference),
            );
            for commit in commits.iter().take(3) {
                if let Some(message) = str_at(commit, &["message"]) {
                    text.push_str("\n• ");
                    text.push_str(first_line(message));
                }
            }
            if total > 3 {
                text.push_str(&format!("\n… and {} more", total - 3));
            }
            Some(Summary {
                title: repo.to_owned(),
                text,
                url: str_at(payload, &["compare"]).map(str::to_owned),
            })
        }
        "pull_request" => {
            let verb = match action {
                "opened" => "opened",
                "reopened" => "reopened",
                "ready_for_review" => "marked ready for review",
                "closed" => {
                    if payload
                        .get("pull_request")
                        .and_then(|p| p.get("merged"))
                        .and_then(Value::as_bool)
                        == Some(true)
                    {
                        "merged"
                    } else {
                        "closed"
                    }
                }
                _ => return None,
            };
            let number = number_at(payload, &["pull_request", "number"])?;
            let title = str_at(payload, &["pull_request", "title"]).unwrap_or("");
            Some(Summary {
                title: repo.to_owned(),
                text: format!("{} {verb} pull request #{number}: {title}", who(payload)),
                url: str_at(payload, &["pull_request", "html_url"]).map(str::to_owned),
            })
        }
        "issues" => {
            let verb = match action {
                "opened" | "reopened" | "closed" => action,
                _ => return None,
            };
            let number = number_at(payload, &["issue", "number"])?;
            let title = str_at(payload, &["issue", "title"]).unwrap_or("");
            Some(Summary {
                title: repo.to_owned(),
                text: format!("{} {verb} issue #{number}: {title}", who(payload)),
                url: str_at(payload, &["issue", "html_url"]).map(str::to_owned),
            })
        }
        "issue_comment" | "pull_request_review_comment" => {
            if action != "created" {
                return None;
            }
            let thing = if event == "issue_comment" {
                "issue"
            } else {
                "pull request"
            };
            let number = number_at(payload, &["issue", "number"])
                .or_else(|| number_at(payload, &["pull_request", "number"]))?;
            let body = str_at(payload, &["comment", "body"])
                .map(first_line)
                .unwrap_or("");
            Some(Summary {
                title: repo.to_owned(),
                text: format!("{} commented on {thing} #{number}\n{body}", who(payload)),
                url: str_at(payload, &["comment", "html_url"]).map(str::to_owned),
            })
        }
        "pull_request_review" => {
            if action != "submitted" {
                return None;
            }
            let state = match str_at(payload, &["review", "state"])? {
                "approved" => "approved",
                "changes_requested" => "requested changes on",
                "commented" => "reviewed",
                _ => return None,
            };
            let number = number_at(payload, &["pull_request", "number"])?;
            Some(Summary {
                title: repo.to_owned(),
                text: format!("{} {state} pull request #{number}", who(payload)),
                url: str_at(payload, &["review", "html_url"]).map(str::to_owned),
            })
        }
        "release" => {
            if action != "published" {
                return None;
            }
            let tag = str_at(payload, &["release", "tag_name"])?;
            let name = str_at(payload, &["release", "name"]).unwrap_or(tag);
            Some(Summary {
                title: repo.to_owned(),
                text: format!("Released {name} ({tag})"),
                url: str_at(payload, &["release", "html_url"]).map(str::to_owned),
            })
        }
        "workflow_run" => {
            if action != "completed" {
                return None;
            }
            // Only the outcomes people act on: a green run is not news.
            let outcome = match str_at(payload, &["workflow_run", "conclusion"])? {
                "failure" => "failed",
                "timed_out" => "timed out",
                _ => return None,
            };
            let name = str_at(payload, &["workflow_run", "name"]).unwrap_or("A workflow");
            let on = str_at(payload, &["workflow_run", "head_branch"]).unwrap_or("a branch");
            Some(Summary {
                title: repo.to_owned(),
                text: format!("{name} {outcome} on {on}"),
                url: str_at(payload, &["workflow_run", "html_url"]).map(str::to_owned),
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn form_encoded_deliveries_are_unwrapped() {
        let body = b"payload=%7B%22zen%22%3A%22Keep+it%22%7D&other=1";
        assert_eq!(form_payload(body).unwrap(), br#"{"zen":"Keep it"}"#);
        assert!(form_payload(b"nothing=1").is_none());
        assert!(form_payload(b"payload=%zz").is_none());
        assert!(form_payload(b"payload=%4").is_none());
    }

    #[test]
    fn push_lists_the_first_commits() {
        let payload = json!({
            "ref": "refs/heads/main", "size": 5,
            "repository": {"full_name": "acme/app"},
            "pusher": {"name": "ana"},
            "compare": "https://github.com/acme/app/compare/a...b",
            "commits": [
                {"message": "Fix login\n\nlong body"}, {"message": "Add tests"},
                {"message": "Bump"}, {"message": "More"}, {"message": "Last"}
            ]
        });
        let s = summarize("push", &payload).unwrap();
        assert_eq!(s.title, "acme/app");
        assert_eq!(
            s.text,
            "ana pushed 5 commits to main\n• Fix login\n• Add tests\n• Bump\n… and 2 more"
        );
        assert!(s.url.unwrap().contains("compare"));
    }

    #[test]
    fn push_of_a_tag_or_with_no_commits_is_ignored() {
        let tag = json!({"ref": "refs/tags/v1", "commits": [{"message": "x"}]});
        assert!(summarize("push", &tag).is_none());
        let empty = json!({"ref": "refs/heads/main", "commits": []});
        assert!(summarize("push", &empty).is_none());
    }

    #[test]
    fn merged_pull_request_and_ignored_actions() {
        let mut payload = json!({
            "action": "closed", "sender": {"login": "luis"},
            "repository": {"full_name": "acme/app"},
            "pull_request": {"number": 7, "title": "Add search", "merged": true,
                "html_url": "https://github.com/acme/app/pull/7"}
        });
        let s = summarize("pull_request", &payload).unwrap();
        assert_eq!(s.text, "luis merged pull request #7: Add search");
        payload["action"] = json!("labeled");
        assert!(summarize("pull_request", &payload).is_none());
    }

    #[test]
    fn only_failed_runs_are_news() {
        let run = |conclusion: &str| {
            json!({"action": "completed", "repository": {"full_name": "acme/app"},
                "workflow_run": {"name": "CI", "conclusion": conclusion, "head_branch": "main"}})
        };
        assert_eq!(
            summarize("workflow_run", &run("failure")).unwrap().text,
            "CI failed on main"
        );
        assert!(summarize("workflow_run", &run("success")).is_none());
    }

    #[test]
    fn ping_confirms_the_connection_and_unknown_events_are_ignored() {
        let ping = json!({"repository": {"full_name": "acme/app"}});
        assert!(summarize("ping", &ping)
            .unwrap()
            .text
            .starts_with("GitHub is connected"));
        assert!(summarize("star", &ping).is_none());
        assert!(summarize("push", &json!(null)).is_none());
    }
}
