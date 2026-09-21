# TypeSafe

Three features ask TypeSafe's System One model (Jev) small questions about your issues: which are about what you searched for, whether a ticket is ready for an agent, and how urgent a label such as `P0` is. All three are off until you turn them on.

## Setup

Each feature needs its setting in config **and** the key in your environment. Either one missing leaves it off and sends nothing.

```
# ~/.config/gh-issues/config.toml
send_issue_text = true         # semantic search and the readiness badge
infer_priority_ranks = true    # priority ranks from label names

export TYPESAFE_API_KEY=...
```

The key is read from the environment only. It is never written to config, and this page never shows it.

The two settings are independent. Turning one on does not turn the other on.

## What leaves your machine

- **`send_issue_text`**: issue text. Semantic search sends your query plus each candidate issue's title and first 200 body characters. The readiness badge sends the title, the first 4000 body characters and the last 10 comments. This is a large disclosure, since private issues are often the most sensitive text in a repo.
- **`infer_priority_ranks`**: label names only.

Never the org name or the repo name, for either setting. One setting covers both text features on purpose: what matters is that issue text leaves, not which feature sends it.

## If something goes wrong

Each feature stops after its first failure and stays off for the session, so a dead connection is not retried on every keypress. Semantic search and priority ranks say so in the status bar and try again after you switch org with `w`. The readiness badge fails silently and tries again after the next refresh. Everything works as it did before either way.

## Trust

Issue text and label names are written by people, and the model can be steered by them. That is why every result here is advisory or bounded: a wrong readiness line is a misleading line, a wrong rank can mis-sort a row, and semantic search only ever adds rows. Nothing acts on a verdict by itself.
