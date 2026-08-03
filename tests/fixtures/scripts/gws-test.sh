#!/bin/sh
if [ "$1" = "--version" ]; then
  echo 'gws 0.99.0-test'
  exit 0
fi

if [ "$1" = "--help" ]; then
  echo 'gws help'
  exit 0
fi

if [ "$1" = "calendar" ] && [ "$2" = "--help" ]; then
  echo 'calendar help'
  exit 0
fi

if [ "$1" = "calendar" ] && [ "$2" = "+insert" ] && [ "$3" = "--help" ]; then
  echo 'calendar insert help'
  exit 0
fi

if [ "$1" = "calendar" ] && [ "$2" = "events" ] && [ "$3" = "delete" ] && [ "$4" = "--help" ]; then
  echo 'calendar delete help'
  exit 0
fi

if [ "$1" = "gmail" ] && [ "$2" = "+triage" ] && [ "$3" = "--help" ]; then
  echo 'gmail triage help'
  exit 0
fi

if [ "$1" = "gmail" ] && [ "$2" = "+read" ] && [ "$3" = "--help" ]; then
  echo 'gmail read help'
  exit 0
fi

if [ "$1" = "calendar" ] && [ "$2" = "+agenda" ]; then
  cat >> "$(dirname "$0")/env.txt" <<EOF
CONFIG_DIR=$GOOGLE_WORKSPACE_CLI_CONFIG_DIR
CREDENTIAL_STORAGE_BACKEND=$GOOGLE_WORKSPACE_CLI_KEYRING_BACKEND
CLIENT_ID=$GOOGLE_WORKSPACE_CLI_CLIENT_ID
CLIENT_SECRET=$GOOGLE_WORKSPACE_CLI_CLIENT_SECRET
CREDENTIALS_FILE=$GOOGLE_WORKSPACE_CLI_CREDENTIALS_FILE
TOKEN=$GOOGLE_WORKSPACE_CLI_TOKEN
ARGV=$*
---
EOF
  cat <<'JSON'
__AGENDA_FIXTURE__
JSON
  exit 0
fi

if [ "$1" = "calendar" ] && [ "$2" = "+insert" ]; then
  cat >> "$(dirname "$0")/env.txt" <<EOF
CONFIG_DIR=$GOOGLE_WORKSPACE_CLI_CONFIG_DIR
CLIENT_ID=$GOOGLE_WORKSPACE_CLI_CLIENT_ID
CLIENT_SECRET=$GOOGLE_WORKSPACE_CLI_CLIENT_SECRET
CREDENTIALS_FILE=$GOOGLE_WORKSPACE_CLI_CREDENTIALS_FILE
TOKEN=$GOOGLE_WORKSPACE_CLI_TOKEN
ARGV=$*
---
EOF
  cat <<'JSON'
__CALENDAR_CREATE_FIXTURE__
JSON
  exit 0
fi

if [ "$1" = "calendar" ] && [ "$2" = "events" ] && [ "$3" = "delete" ]; then
  cat >> "$(dirname "$0")/env.txt" <<EOF
CONFIG_DIR=$GOOGLE_WORKSPACE_CLI_CONFIG_DIR
CLIENT_ID=$GOOGLE_WORKSPACE_CLI_CLIENT_ID
CLIENT_SECRET=$GOOGLE_WORKSPACE_CLI_CLIENT_SECRET
CREDENTIALS_FILE=$GOOGLE_WORKSPACE_CLI_CREDENTIALS_FILE
TOKEN=$GOOGLE_WORKSPACE_CLI_TOKEN
ARGV=$*
---
EOF
  cat <<'JSON'
__CALENDAR_DELETE_FIXTURE__
JSON
  exit 0
fi

if [ "$1" = "gmail" ] && [ "$2" = "+triage" ]; then
  cat >> "$(dirname "$0")/env.txt" <<EOF
CONFIG_DIR=$GOOGLE_WORKSPACE_CLI_CONFIG_DIR
CLIENT_ID=$GOOGLE_WORKSPACE_CLI_CLIENT_ID
CLIENT_SECRET=$GOOGLE_WORKSPACE_CLI_CLIENT_SECRET
CREDENTIALS_FILE=$GOOGLE_WORKSPACE_CLI_CREDENTIALS_FILE
TOKEN=$GOOGLE_WORKSPACE_CLI_TOKEN
ARGV=$*
---
EOF
  cat <<'JSON'
__GMAIL_TRIAGE_FIXTURE__
JSON
  exit 0
fi

if [ "$1" = "gmail" ] && [ "$2" = "+read" ]; then
  cat >> "$(dirname "$0")/env.txt" <<EOF
CONFIG_DIR=$GOOGLE_WORKSPACE_CLI_CONFIG_DIR
CLIENT_ID=$GOOGLE_WORKSPACE_CLI_CLIENT_ID
CLIENT_SECRET=$GOOGLE_WORKSPACE_CLI_CLIENT_SECRET
CREDENTIALS_FILE=$GOOGLE_WORKSPACE_CLI_CREDENTIALS_FILE
TOKEN=$GOOGLE_WORKSPACE_CLI_TOKEN
ARGV=$*
---
EOF
  cat <<'JSON'
__GMAIL_READ_FIXTURE__
JSON
  exit 0
fi

if [ "$1" = "gmail" ] && [ "$2" = "users" ] && [ "$3" = "drafts" ] && [ "$4" = "create" ]; then
  cat >> "$(dirname "$0")/env.txt" <<EOF
CONFIG_DIR=$GOOGLE_WORKSPACE_CLI_CONFIG_DIR
CLIENT_ID=$GOOGLE_WORKSPACE_CLI_CLIENT_ID
CLIENT_SECRET=$GOOGLE_WORKSPACE_CLI_CLIENT_SECRET
CREDENTIALS_FILE=$GOOGLE_WORKSPACE_CLI_CREDENTIALS_FILE
TOKEN=$GOOGLE_WORKSPACE_CLI_TOKEN
ARGV=$*
---
EOF
  cat <<'JSON'
__GMAIL_DRAFT_CREATE_FIXTURE__
JSON
  exit 0
fi

if [ "$1" = "auth" ] && [ "$2" = "login" ]; then
  cat >> "$(dirname "$0")/env.txt" <<EOF
CONFIG_DIR=$GOOGLE_WORKSPACE_CLI_CONFIG_DIR
CLIENT_ID=$GOOGLE_WORKSPACE_CLI_CLIENT_ID
CLIENT_SECRET=$GOOGLE_WORKSPACE_CLI_CLIENT_SECRET
CREDENTIALS_FILE=$GOOGLE_WORKSPACE_CLI_CREDENTIALS_FILE
TOKEN=$GOOGLE_WORKSPACE_CLI_TOKEN
ARGV=$*
---
EOF
  echo "Open this URL in your browser to authenticate:"
  echo "https://accounts.google.com/o/oauth2/auth?test=1"
  exit 0
fi

if [ "$1" = "auth" ] && [ "$2" = "status" ]; then
  cat >> "$(dirname "$0")/env.txt" <<EOF
CONFIG_DIR=$GOOGLE_WORKSPACE_CLI_CONFIG_DIR
CLIENT_ID=$GOOGLE_WORKSPACE_CLI_CLIENT_ID
CLIENT_SECRET=$GOOGLE_WORKSPACE_CLI_CLIENT_SECRET
CREDENTIALS_FILE=$GOOGLE_WORKSPACE_CLI_CREDENTIALS_FILE
TOKEN=$GOOGLE_WORKSPACE_CLI_TOKEN
ARGV=$*
---
EOF
  cat <<JSON
{"user":"__AUTH_STATUS_USER__"}
JSON
  exit 0
fi

echo "unexpected args: $*" >&2
exit 1
