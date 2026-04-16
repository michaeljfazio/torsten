---
name: gh release create flag
description: gh release create uses --notes not --body for release body text
type: feedback
---

Use `--notes` (not `--body`) when passing release notes text to `gh release create`.

**Why:** `--body` is not a valid flag for `gh release create` in the version installed in this repo's environment — it errors with "unknown flag: --body". The correct flag is `--notes` for inline text or `--notes-file` for a file.

**How to apply:** Always use `--notes "..."` or `--notes-file <path>` when creating GitHub releases via `gh release create`.
