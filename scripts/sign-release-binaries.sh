#!/usr/bin/env bash
set -euo pipefail

signing_dir=$(mktemp -d "$RUNNER_TEMP/native-signing.XXXXXX")
keychain_path="$signing_dir/app-signing.keychain-db"
cleanup() {
  security delete-keychain "$keychain_path" || true
  rm -rf "$signing_dir"
}
trap cleanup EXIT

keychain_password=$(openssl rand -base64 32)
security create-keychain -p "$keychain_password" "$keychain_path"
security set-keychain-settings -lut 21600 "$keychain_path"
security unlock-keychain -p "$keychain_password" "$keychain_path"
printf '%s' "$CERTIFICATE_BASE64" | base64 --decode > "$signing_dir/certificate.p12"
security import "$signing_dir/certificate.p12" \
  -P "$CERTIFICATE_PASSWORD" -T /usr/bin/codesign -T /usr/bin/security \
  -f pkcs12 -k "$keychain_path"
rm "$signing_dir/certificate.p12"
security set-key-partition-list -S apple-tool:,apple:,codesign: \
  -s -k "$keychain_password" "$keychain_path"

cat > "$signing_dir/entitlements.plist" << 'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN">
<plist version="1.0">
  <dict>
    <key>com.apple.security.cs.allow-unsigned-executable-memory</key>
    <true/>
  </dict>
</plist>
EOF

for project in golem-cli golem; do
  binary_path="release-native/$project-$RELEASE_TARGET"
  codesign --force --options runtime --timestamp \
    --keychain "$keychain_path" \
    --entitlements "$signing_dir/entitlements.plist" \
    --sign "$APPLE_SIGNING_IDENTITY" "$binary_path"
  codesign --verify --verbose "$binary_path"
  codesign --display --verbose=4 --entitlements - "$binary_path"
  zip -j "$signing_dir/$project.zip" "$binary_path"
  xcrun notarytool submit "$signing_dir/$project.zip" \
    --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" \
    --password "$APPLE_APP_PASSWORD" --wait --timeout 30m \
    --output-format json > "$signing_dir/$project-result.json"
  python3 - "$signing_dir/$project-result.json" << 'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as result_file:
    result = json.load(result_file)
if result.get("status") != "Accepted":
    raise SystemExit(f"Notarization was not Accepted: {result}")
print(f"Notarization Accepted: {result['id']}")
PY
done
