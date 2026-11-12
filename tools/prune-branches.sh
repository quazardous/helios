#!/usr/bin/env bash
# Lists, then deletes, the branches whose work is already merged: local
# branches and the remote's (origin). Works in whatever git repository it
# runs from, so the sibling repositories can use it too.
#
#   tools/prune-branches.sh                  # dry run: what would go
#   tools/prune-branches.sh --apply          # delete them
#   tools/prune-branches.sh --days 0         # no age limit
#   tools/prune-branches.sh --local-only     # leave origin alone
#
# A branch counts as merged when its tip is in origin/next or origin/main,
# or when it is the head of a merged pull request at that same commit
# (squash and rebase merges leave no ancestry; a branch that moved after
# its PR merged is kept). Never touched: main, next, master, the current
# branch, a branch checked out in any worktree, the head of an open pull
# request, and branches whose last commit is younger than --days (default
# 3). Pull request data needs the GitHub CLI (`gh`); without it only the
# ancestry check runs.
set -euo pipefail

apply=0 days=3 remote=1
while [ $# -gt 0 ]; do
  case "$1" in
    --apply) apply=1 ;;
    --days) days=$2; shift ;;
    --local-only) remote=0 ;;
    -h|--help) sed -n '2,19p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done

git fetch --quiet --prune origin
now=$(date +%s)
cutoff=$((now - days * 86400))

declare -A keep merged_pr open_pr
for b in main next master HEAD; do keep[$b]=1; done
cur=$(git branch --show-current || true)
[ -n "$cur" ] && keep[$cur]=1
while read -r line; do
  case "$line" in branch\ refs/heads/*) keep[${line#branch refs/heads/}]=1 ;; esac
done < <(git worktree list --porcelain)

if command -v gh >/dev/null && gh repo view >/dev/null 2>&1; then
  while IFS=$'\t' read -r name sha; do merged_pr[$name]=$sha; done < <(
    gh pr list --state merged --limit 1000 --json headRefName,headRefOid \
      -q '.[] | [.headRefName, .headRefOid] | @tsv')
  while read -r name; do open_pr[$name]=1; done < <(
    gh pr list --state open --limit 1000 --json headRefName -q '.[].headRefName')
else
  echo "(gh unavailable: squash-merged branches are not detected)" >&2
fi

bases=()
for b in origin/next origin/main; do
  git rev-parse --verify --quiet "$b" >/dev/null && bases+=("$b")
done

is_merged() { # name sha
  local base
  for base in "${bases[@]}"; do
    git merge-base --is-ancestor "$2" "$base" && return 0
  done
  [ "${merged_pr[$1]:-}" = "$2" ]
}

candidates() { # ref-prefix strip
  git for-each-ref --format='%(refname)%09%(objectname)%09%(committerdate:unix)' "$1" |
  while IFS=$'\t' read -r ref sha when; do
    name=${ref#"$2"}
    [ -n "${keep[$name]:-}" ] && continue
    [ -n "${open_pr[$name]:-}" ] && continue
    [ "$when" -gt "$cutoff" ] && continue
    is_merged "$name" "$sha" || continue
    echo "$name"
  done
}

mapfile -t local_gone < <(candidates refs/heads/ refs/heads/)
remote_gone=()
if [ $remote = 1 ]; then
  mapfile -t remote_gone < <(candidates refs/remotes/origin/ refs/remotes/origin/)
fi

echo "local (${#local_gone[@]}):";  printf '  %s\n' "${local_gone[@]}"
[ $remote = 1 ] && { echo "origin (${#remote_gone[@]}):"; printf '  %s\n' "${remote_gone[@]}"; }

if [ $apply = 0 ]; then
  echo "dry run; --apply deletes them"
  exit 0
fi
for b in "${local_gone[@]}"; do git branch -D "$b" >/dev/null && echo "deleted $b"; done
if [ ${#remote_gone[@]} -gt 0 ]; then
  # One push per 50 branches keeps the command line short.
  for ((i = 0; i < ${#remote_gone[@]}; i += 50)); do
    git push --quiet origin --delete "${remote_gone[@]:i:50}"
  done
  echo "deleted ${#remote_gone[@]} branches on origin"
fi
