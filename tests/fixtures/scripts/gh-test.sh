#!/bin/sh
if [ "$1" = "--version" ]; then
  echo 'gh version 9.9.9-test'
  exit 0
fi

if [ "$1" = "--help" ]; then
  echo 'gh help'
  exit 0
fi

if [ "$1" = "help" ] && [ "$2" = "pr" ] && [ "$3" = "view" ]; then
  echo 'pr view help'
  exit 0
fi

if [ "$1" = "help" ] && [ "$2" = "repo" ] && [ "$3" = "view" ]; then
  echo 'repo view help'
  exit 0
fi

if [ "$1" = "api" ] && [ "$2" = "--help" ]; then
  echo 'api help'
  exit 0
fi

if [ "$1" = "search" ] && [ "$2" = "prs" ] && [ "$3" = "--help" ]; then
  echo 'search prs help'
  exit 0
fi

if [ "$1" = "search" ] && [ "$2" = "repos" ] && [ "$3" = "--help" ]; then
  echo 'search repos help'
  exit 0
fi

if [ "$1" = "pr" ] && [ "$2" = "view" ] && [ "$3" = "--help" ]; then
  echo 'pr view help'
  exit 0
fi

if [ "$1" = "issue" ] && [ "$2" = "view" ] && [ "$3" = "--help" ]; then
  echo 'issue view help'
  exit 0
fi

if [ "$1" = "api" ] && [ "$2" = "notifications" ]; then
  cat >> "$(dirname "$0")/env.txt" <<EOF
GH_CONFIG_DIR=$GH_CONFIG_DIR
GH_TOKEN=$GH_TOKEN
GITHUB_TOKEN=$GITHUB_TOKEN
ARGV=$*
---
EOF
  cat <<'JSON'
__NOTIFICATIONS_FIXTURE__
JSON
  exit 0
fi

if [ "$1" = "search" ] && [ "$2" = "prs" ]; then
  cat >> "$(dirname "$0")/env.txt" <<EOF
GH_CONFIG_DIR=$GH_CONFIG_DIR
GH_TOKEN=$GH_TOKEN
GITHUB_TOKEN=$GITHUB_TOKEN
ARGV=$*
---
EOF
  cat <<'JSON'
__PR_SEARCH_FIXTURE__
JSON
  exit 0
fi

if [ "$1" = "search" ] && [ "$2" = "repos" ]; then
  cat >> "$(dirname "$0")/env.txt" <<EOF
GH_CONFIG_DIR=$GH_CONFIG_DIR
GH_TOKEN=$GH_TOKEN
GITHUB_TOKEN=$GITHUB_TOKEN
ARGV=$*
---
EOF
  cat <<'JSON'
__REPOSITORY_SEARCH_FIXTURE__
JSON
  exit 0
fi

if [ "$1" = "pr" ] && [ "$2" = "view" ]; then
  cat >> "$(dirname "$0")/env.txt" <<EOF
GH_CONFIG_DIR=$GH_CONFIG_DIR
GH_TOKEN=$GH_TOKEN
GITHUB_TOKEN=$GITHUB_TOKEN
ARGV=$*
---
EOF
  cat <<'JSON'
__PR_READ_FIXTURE__
JSON
  exit 0
fi

if [ "$1" = "issue" ] && [ "$2" = "view" ]; then
  cat >> "$(dirname "$0")/env.txt" <<EOF
GH_CONFIG_DIR=$GH_CONFIG_DIR
GH_TOKEN=$GH_TOKEN
GITHUB_TOKEN=$GITHUB_TOKEN
ARGV=$*
---
EOF
  cat <<'JSON'
__ISSUE_READ_FIXTURE__
JSON
  exit 0
fi

if [ "$1" = "repo" ] && [ "$2" = "view" ]; then
  cat >> "$(dirname "$0")/env.txt" <<EOF
GH_CONFIG_DIR=$GH_CONFIG_DIR
GH_TOKEN=$GH_TOKEN
GITHUB_TOKEN=$GITHUB_TOKEN
ARGV=$*
---
EOF
  cat <<'JSON'
__REPO_VIEW_FIXTURE__
JSON
  exit 0
fi

echo "unexpected args: $*" >&2
exit 1
