# Troubleshooting

Something went wrong? Pick the entry point that matches your situation:

## By shape of the problem

| Situation | Go to |
|---|---|
| "I have an **error message** in front of me and I want the fix." | [`symptoms.md`](./symptoms.md) — alphabetical index of every verbatim error string the `makakoo` binary emits. |
| "I observe a **behavior** (something doesn't work, something feels wrong) but I don't have a specific error string." | [`tree.md`](./tree.md) — symptom-rooted decision tree; pick the top-level category and drill down. |
| "I want to **uninstall** / wipe / reinstall from scratch." | [`uninstall.md`](./uninstall.md). |
| "I read the full flat guide before the sprint split it up, and I want it back." | [`legacy-full.md`](./legacy-full.md) — the pre-split prose reference. Not actively maintained; use it as a secondary source if the tree / symptoms pages don't have what you need. |

## Fast self-check

Before diving in, run these three commands. If all three print without errors, your infrastructure is alive and the problem is narrower than "makakoo is broken":

```sh
makakoo version           # binary + persona + home
makakoo sancho status     # task engine alive
makakoo memory stats      # memory layer alive
```

If `makakoo` is installed and all three work, jump straight to [`tree.md`](./tree.md) and pick the category that matches your observed behavior.

## If you still can't find it

- Search `legacy-full.md` by `Ctrl+F`.
- File an issue with: `makakoo version` output + the exact command + the exact error + one line of what you expected.
- Regular readers should help this page stay useful: if you solve a problem that wasn't covered, add the symptom to `tree.md` and (if there's a verbatim string) to `symptoms.md`.
