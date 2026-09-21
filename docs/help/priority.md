# Priority ranks

Priority sort, title colouring and the priority filter all understand labels like `priority:high`. A repo that labels priority `P0`/`P1`, `sev1`, `blocker` or `nice-to-have` has none of those, so every issue used to rank the same and sorting by priority did nothing.

With inference on, TypeSafe rates each label name it does not know on the same scale, from 1 (low) to 4 (urgent). The rank is used to:

- sort by priority (`s`) and colour titles;
- order the priority picker in the filter editor (`F`);
- fill the `p` (set priority) picker on a repo that has no `priority:*` label.

## Turning it on

Both are needed. See the typesafe page.

```
# ~/.config/gh-issues/config.toml
infer_priority_ranks = true

export TYPESAFE_API_KEY=...
```

Without both, nothing changes and nothing is sent.

## What is sent

Label names only. Never titles, bodies, numbers, URLs or the org name. A label name can still be sensitive on a private org, which is why this is opt-in.

## Setting a priority with `p`

- On a repo that uses `priority:*` labels, `p` works exactly as it always did and no inferred rank is involved.
- On a repo that does not, `p` offers that repo's own ranked labels. Setting one replaces the issue's current priority label, so if that would remove a label the model ranked, you are asked first. The popup names each label and defaults to No.

## Good to know

- The model must be at least 0.7 confident, or the label stays unranked, which is what happened before inference existed. Odd names like `soon` or `parked` may or may not rank.
- Answers are cached per org in `~/.cache/gh-issues/label-ranks.json`, so each label is asked about once.
- If asking fails, you get one status message and inference stays off until you switch org with `w`.
- `status:*` labels are not ranked: status has no order to infer.
