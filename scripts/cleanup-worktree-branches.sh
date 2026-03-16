#!/usr/bin/env bash
# cleanup-worktree-branches.sh
#
# Removes all worktree-agent-* branches and their associated worktrees.
# This script MUST be run from the main repo directory (not from a worktree).
#
# Usage:
#   cd /Users/michaelfazio/Source/torsten
#   bash scripts/cleanup-worktree-branches.sh
#
# What it does:
#   1. Removes all .claude/worktrees/agent-* git worktrees (git worktree remove --force)
#   2. Prunes any stale worktree references
#   3. Deletes all local worktree-agent-* branches
#
# Safety:
#   - Does NOT touch remote branches
#   - Does NOT touch the main branch or chore/* branches
#   - Uses --force on worktree remove to handle dirty worktrees
#   - Skips any branch that cannot be deleted (e.g., currently checked out)

set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"

echo "=== Worktree Agent Branch Cleanup ==="
echo "Repo: $REPO_ROOT"
echo ""

# Step 1: Remove all agent worktrees
echo "--- Step 1: Removing agent worktrees ---"
git worktree list --porcelain | grep '^worktree ' | while read -r _ path; do
    if [[ "$path" == */.claude/worktrees/agent-* ]]; then
        echo "  Removing worktree: $path"
        git worktree remove --force "$path" 2>/dev/null || echo "  WARN: Could not remove $path"
    fi
done

# Step 2: Prune stale worktree references
echo ""
echo "--- Step 2: Pruning stale worktree references ---"
git worktree prune -v

# Step 3: Delete all worktree-agent-* branches
echo ""
echo "--- Step 3: Deleting worktree-agent-* branches ---"
BRANCHES=$(git branch --list 'worktree-agent-*' | sed 's/^[* +]*//')
BRANCH_COUNT=$(echo "$BRANCHES" | grep -c . || true)
echo "  Found $BRANCH_COUNT worktree-agent-* branches"

echo "$BRANCHES" | while read -r branch; do
    if [ -n "$branch" ]; then
        echo "  Deleting branch: $branch"
        git branch -D "$branch" 2>/dev/null || echo "  WARN: Could not delete $branch"
    fi
done

echo ""
echo "=== Cleanup complete ==="
echo "Remaining worktrees:"
git worktree list
echo ""
echo "Remaining worktree-agent branches:"
git branch --list 'worktree-agent-*' | wc -l | xargs echo "  Count:"
