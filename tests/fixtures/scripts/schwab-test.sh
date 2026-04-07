#!/bin/sh
if [ "$1" = "--version" ]; then
  echo 'schwab 0.1.0-test'
  exit 0
fi

if [ "$1" = "--help" ]; then
  echo 'schwab help'
  exit 0
fi

if [ "$1" = "auth" ] && [ "$2" = "--help" ]; then
  echo 'auth help'
  exit 0
fi

if [ "$1" = "auth" ] && [ "$2" = "status" ] && [ "$3" = "--help" ]; then
  echo 'auth status help'
  exit 0
fi

if [ "$1" = "auth" ] && [ "$2" = "status" ]; then
  cat >> "$(dirname "$0")/env.txt" <<EOF
CONFIG=$SCHWAB_CONFIG
BASE_URL=$SCHWAB_BASE_URL
MARKET_DATA_BASE_URL=$SCHWAB_MARKETDATA_BASE_URL
AUTHORIZE_URL=$SCHWAB_AUTHORIZE_URL
TOKEN_URL=$SCHWAB_TOKEN_URL
CLIENT_ID=$SCHWAB_CLIENT_ID
CLIENT_SECRET=$SCHWAB_CLIENT_SECRET
THIRD_PARTY_ID=$SCHWAB_THIRD_PARTY_ID
CLIENT_CHANNEL=$SCHWAB_TRADER_CLIENT_CHANNEL
CLIENT_APP_ID=$SCHWAB_TRADER_CLIENT_APP_ID
CLIENT_FUNCTION_ID=$SCHWAB_CLIENT_FUNCTION_ID
RESOURCE_VERSION=$SCHWAB_RESOURCE_VERSION
RRBUS_PILOT_ROLLOUT=$SCHWAB_RRBUS_PILOT_ROLLOUT
REDIRECT_URI=$SCHWAB_REDIRECT_URI
ACCESS_TOKEN=$SCHWAB_ACCESS_TOKEN
REFRESH_TOKEN=$SCHWAB_REFRESH_TOKEN
ARGV=$*
---
EOF
  cat <<'JSON'
{"authenticated":true}
JSON
  exit 0
fi

echo "unexpected args: $*" >&2
exit 1
