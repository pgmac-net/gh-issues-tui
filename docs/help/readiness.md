# Readiness badge

A coding-agent run (`A`) is expensive, and a vague ticket, a question filed as work, or something already covered elsewhere all burn one. With TypeSafe on, the detail pane shows one extra line:

```
readiness: ready — is specific and states an outcome
```

## What the line can say

- `ready` the ticket is specific and states what done looks like.
- `thin — no specifics` or `no criteria` something is missing.
- `blocked` the thread says it is waiting on something unresolved.
- `already covered` the thread says it is done elsewhere.
- `not a work item` it reads as a question or an update.
- `may be blocked` or `may be a duplicate`, `worth checking` the thread hints at it. Go and read it.
- `unclear — cannot judge …` the model would not commit.

The lines that say something may be wrong (`blocked`, `already covered`, `not a work item` and both `may be` ones) take the warning colour. `thin` and `unclear` are dim.

## Turning it on

Both are needed. See the typesafe page.

```
# ~/.config/gh-issues/config.toml
send_issue_text = true

export TYPESAFE_API_KEY=...
```

Without both, there is no badge and nothing is sent. The line appears once the ticket's comments have loaded, so it may take a moment after opening an issue.

## What is sent

The title, the first 4000 characters of the body, the last 10 comments cut to 500 characters each, and the true comment count. Never the org or repo name. Nothing is written to disk; answers live for the session.

## Good to know

- **It is advice only.** It cannot change, delay or block launching an agent. The ticket text is written by other people and can push the model around, so nothing acts on the verdict.
- **Treat it as unproven.** Checked on 22 public tickets it still hedged on 7. `ready` is the most reliable verdict; `unclear` mostly means the ticket is borderline.
- If asking fails, the line just does not appear. There is no message. A refresh tries again.
