---
name: GitHub Issue Workflow
description: Exact process for addressing GitHub issues — branch, PR, merge, close
type: feedback
---

When working on GitHub issues, follow this exact workflow:
1. **Read** the issue details thoroughly
2. **Create a branch** named against the issue (e.g., `fix/117-n2c-encoding`)
3. **Do all work** on that branch — commits, tests, fixes
4. **Confirm** the fix/change works (tests pass, clippy clean, fmt clean)
5. **Push** the branch to remote
6. **Create a pull request** against main
7. **Update the issue** with details if necessary
8. If we own the repository, **merge the PR** into mainline
9. **Close the issue**

**Why:** The user wants a clean, traceable workflow — issue → branch → PR → merge → close. No direct commits to main for issue work.

**How to apply:** Every time you pick up a GitHub issue, follow all 9 steps in order. Never commit issue work directly to main. Always go through a PR.
