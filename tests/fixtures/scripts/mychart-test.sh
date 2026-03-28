#!/bin/sh
if [ "$1" = "--version" ]; then
  echo 'mychart 0.1.0-test'
  exit 0
fi

if [ "$1" = "--help" ]; then
  echo 'mychart help'
  exit 0
fi

if [ "$1" = "appointments" ] && [ "$2" = "--help" ]; then
  echo 'appointments help'
  exit 0
fi

if [ "$1" = "appointments" ] && [ "$2" = "upcoming" ] && [ "$3" = "--help" ]; then
  echo 'appointments upcoming help'
  exit 0
fi

if [ "$1" = "notes" ] && [ "$2" = "--help" ]; then
  echo 'notes help'
  exit 0
fi

if [ "$1" = "notes" ] && [ "$2" = "search" ] && [ "$3" = "--help" ]; then
  echo 'notes search help'
  exit 0
fi

if [ "$1" = "appointments" ] && [ "$2" = "upcoming" ]; then
  cat >> "$(dirname "$0")/env.txt" <<EOF
CONFIG=$MYCHART_CONFIG
ACCOUNT=$MYCHART_ACCOUNT
BASE_URL=$MYCHART_BASE_URL
CLIENT_ID=$MYCHART_CLIENT_ID
CLIENT_SECRET=$MYCHART_CLIENT_SECRET
REDIRECT_URI=$MYCHART_REDIRECT_URI
ACCESS_TOKEN=$MYCHART_ACCESS_TOKEN
REFRESH_TOKEN=$MYCHART_REFRESH_TOKEN
USERNAME=$MYCHART_USERNAME
ARGV=$*
---
EOF
  cat <<'JSON'
__APPOINTMENTS_UPCOMING_FIXTURE__
JSON
  exit 0
fi

if [ "$1" = "notes" ] && [ "$2" = "search" ]; then
  cat >> "$(dirname "$0")/env.txt" <<EOF
CONFIG=$MYCHART_CONFIG
ACCOUNT=$MYCHART_ACCOUNT
BASE_URL=$MYCHART_BASE_URL
CLIENT_ID=$MYCHART_CLIENT_ID
CLIENT_SECRET=$MYCHART_CLIENT_SECRET
REDIRECT_URI=$MYCHART_REDIRECT_URI
ACCESS_TOKEN=$MYCHART_ACCESS_TOKEN
REFRESH_TOKEN=$MYCHART_REFRESH_TOKEN
USERNAME=$MYCHART_USERNAME
ARGV=$*
---
EOF
  cat <<'JSON'
__NOTES_SEARCH_FIXTURE__
JSON
  exit 0
fi

echo "unexpected args: $*" >&2
exit 1
