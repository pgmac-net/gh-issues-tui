# Search

`/` narrows the list to issues whose title or body contains your text, or whose number matches. Sort order is untouched.

With TypeSafe on, `/` also finds issues that are **about** what you typed, even in different words. Searching `login token expiry` can find an issue titled `auth cookie lifetime is off by one`. Text matches appear at once; the extra issues join them a few seconds later. Nothing is ever taken away: `#123`, an exact error string or a hostname still match as before.

## Turning it on

Both are needed. See the typesafe page for the details.

```
# ~/.config/gh-issues/config.toml
send_issue_text = true

export TYPESAFE_API_KEY=...
```

Without both, `/` is the plain text match, and nothing leaves your machine.

## What is sent

For every issue the other filters let through, one request with your query and that issue's title and the first 200 characters of its body. Never the org, the repo, the issue number or the key. An empty query or a bare number (`123`, `#123`) sends nothing.

## Telling it is working

While a query is typed, the info bar at the bottom says:

- `semantic: searching…` a request is out.
- `semantic: +3` it landed and added 3 issues the text match did not show. `+0` means nothing else is about it.
- `semantic: off (reason)` asking failed; `/` is text-only until you switch org with `w`, which tries again.

Nothing is shown without the settings above, or for a query that is never sent.

## Good to know

- An issue counts as a match when the model rates it above 0.70. That errs toward missing a loosely related issue over adding an unrelated one, but a stray related issue can still appear.
- It takes about 4 to 6 seconds and costs about $0.003 per search.
- Relaxing a filter, showing closed issues or a refresh that brings new issues searches again. Narrowing a filter never does, because every issue left was already judged.
- Only `/` and the filter editor's text field search this way.
