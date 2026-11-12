#!/usr/bin/env bash
# Probe Hollyland's firmware-check API the way HollyAudio calls it (docs/APK-NOTES.md).
# Answers two open questions in docs/WIP.md:
#   1. Is the request format right? Other Hollyland products should get real offers.
#   2. Does the server check the signature? A request with a wrong one should fail.
# Read-only: it only asks which firmware exists; it downloads nothing. No serial numbers.
set -euo pipefail
trap 'printf "\n🔴 FAILED\n" >&2' ERR

base=https://device-api.hollyland.com
fail() { printf '🔴 %s\n' "$*" >&2; printf '\n🔴 FAILED\n' >&2; exit 1; }

key_json=$(curl -fsS --max-time 15 "$base/api/v1/sign/getSignKey") || fail "getSignKey: no answer"
key=$(sed -n 's/.*"data":"\([^"]*\)".*/\1/p' <<<"$key_json")
[[ -n $key ]] || fail "getSignKey: unexpected reply $key_json"
printf '🟢 sign key: %s\n' "$key"

post() { # path body [sign-override]
  local ts sign
  ts=$(($(date +%s) * 1000))
  sign=${3:-$(printf '%s' "$key$ts$2$key" | md5sum | cut -d' ' -f1)}
  curl -sS --max-time 15 -X POST "$base$1" -H 'Content-Type: application/json' \
    -H 'APP-PKG-CODE: com.hollyland.larkc1' -H 'APP-VERSION-VALUE: 3101020' \
    --data-binary "{\"sign\":\"$sign\",\"timestamp\":$ts,\"body\":$2}"
  echo
}

codes=(6301RX 6301TX 6302RX 6302TX 6118RX 6118TX 6118CAMRX 6301RX_RU)
body="[$(printf '{"packageCode":"%s","versionValue":1000},' "${codes[@]}" | sed 's/,$//')]"
printf '\n🌱 checkBatchUpgrade for %s\n' "${codes[*]}"
reply=$(post /api/v1/upgrade/checkBatchUpgrade "$body")
echo "$reply"

printf '\n🌱 same request with a wrong signature\n'
post /api/v1/upgrade/checkBatchUpgrade "$body" 00000000000000000000000000000000

printf '\n🌱 offered versions\n'
grep -o '"packageCode":"[^"]*","versionName":"[^"]*"' <<<"$reply" | sed 's/"packageCode":"//; s/","versionName":"/ → /; s/"$//' || echo "nothing offered"
printf '\n🟢 SUCCESS: paste the output above back into the chat\n'
