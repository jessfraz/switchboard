#!/bin/sh
if [ "$1" = "--version" ]; then
  echo 'gh version 9.9.9-test'
  exit 0
fi

if [ "$1" = "api" ] && [ "$2" = "--help" ]; then
  echo 'api help'
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

echo "unexpected args: $*" >&2
exit 1
