# macOS Code Signing Setup for CI

This document explains how to set up the required GitHub secrets for macOS code signing and notarization in the CI/CD pipeline.

## Prerequisites

1. An Apple Developer account with a valid "Developer ID Application" certificate
2. Access to the GitHub repository secrets settings
3. macOS machine for generating the certificate (or access to existing certificate)

## Required GitHub Secrets

The following secrets must be added to the GitHub repository at Settings → Secrets and variables → Actions:

### 1. APPLE_CERTIFICATE_BASE64

This is your Developer ID Application certificate exported as a base64-encoded .p12 file.

**How to generate (recommended GUI method):**

```bash
# Step 1: Export certificate from Keychain Access
# 1. Open Keychain Access app
# 2. Select "login" keychain on the left
# 3. Select "My Certificates" category
# 4. Find your "Developer ID Application" certificate
# 5. Right-click → Export "Developer ID Application: ..."
# 6. Choose file format: "Personal Information Exchange (.p12)"
# 7. Save to Desktop as 'certificate.p12'
# 8. Enter an export password when prompted (remember this!)

# Step 2: Convert to base64 and copy to clipboard
base64 -i ~/Desktop/certificate.p12 | pbcopy

# Step 3: Clean up the certificate file
rm ~/Desktop/certificate.p12

# The base64 string is now in your clipboard
# Paste it as the APPLE_CERTIFICATE_BASE64 secret
```

**If you get "Operation not permitted" on the base64 command:**

```bash
# First, verify the file exists and check its location
ls -la ~/Desktop/certificate.p12

# Remove macOS quarantine attributes (common issue)
xattr -d com.apple.quarantine ~/Desktop/certificate.p12 2>/dev/null || true

# Fix permissions
chmod 644 ~/Desktop/certificate.p12

# Try using cat instead of base64 -i (more reliable on macOS)
cat ~/Desktop/certificate.p12 | base64 | pbcopy

# Clean up
rm ~/Desktop/certificate.p12
```

**Alternative: Use /tmp directory (avoids Desktop permissions issues):**

```bash
# Export to /tmp instead of Desktop when using Keychain Access GUI
# Then run:
cat /tmp/certificate.p12 | base64 | pbcopy
rm /tmp/certificate.p12
```

**Alternative using security command line:**

```bash
# Find your certificate identity
security find-identity -v -p codesigning

# Export to file (replace YOUR_EXPORT_PASSWORD with a password you choose)
security export -t identities -f pkcs12 \
  -k ~/Library/Keychains/login.keychain-db \
  -P "YOUR_EXPORT_PASSWORD" \
  -o ~/Desktop/certificate.p12

# Fix permissions if needed
chmod 644 ~/Desktop/certificate.p12

# Convert to base64 and copy to clipboard
base64 -i ~/Desktop/certificate.p12 | pbcopy

# Clean up the certificate file
rm ~/Desktop/certificate.p12
```

### 2. APPLE_CERTIFICATE_PASSWORD

The password you used when exporting the .p12 certificate file.

**Value:** The password string (e.g., `MySecurePassword123`)

### 3. APPLE_ID

Your Apple ID email address used for notarization.

**Value:** Your Apple ID email (e.g., `developer@example.com`)

### 4. APPLE_TEAM_ID

Your Apple Developer Team ID.

**How to find:**

1. Visit https://developer.apple.com/account
2. Log in with your Apple ID
3. Your Team ID is displayed in the top right corner
4. Or run: `xcrun altool --list-providers -u "YOUR_APPLE_ID" -p "APP_PASSWORD"`

**Value:** 10-character alphanumeric string (e.g., `A1B2C3D4E5`)

### 5. APPLE_APP_PASSWORD

An app-specific password for notarization (not your regular Apple ID password).

**How to generate:**

1. Visit https://appleid.apple.com/account/manage
2. Log in with your Apple ID
3. Navigate to "Sign-In and Security" → "App-Specific Passwords"
4. Click "Generate an app-specific password"
5. Enter a name (e.g., "GitHub Actions Notarization")
6. Copy the generated password (format: `xxxx-xxxx-xxxx-xxxx`)

**Value:** The app-specific password (e.g., `abcd-efgh-ijkl-mnop`)

## Setting Up Secrets in GitHub

1. Navigate to your repository on GitHub
2. Go to Settings → Secrets and variables → Actions
3. Click "New repository secret"
4. Add each secret with its corresponding name and value:
   - Name: `APPLE_CERTIFICATE_BASE64`, Value: [base64 certificate]
   - Name: `APPLE_CERTIFICATE_PASSWORD`, Value: [certificate password]
   - Name: `APPLE_ID`, Value: [your Apple ID]
   - Name: `APPLE_TEAM_ID`, Value: [your team ID]
   - Name: `APPLE_APP_PASSWORD`, Value: [app-specific password]

## Obtaining a Developer ID Certificate

If you don't have a Developer ID Application certificate:

1. Visit https://developer.apple.com/account/resources/certificates/list
2. Click the "+" button to create a new certificate
3. Select "Developer ID Application" under "Developer ID"
4. Follow the instructions to create a Certificate Signing Request (CSR)
5. Upload the CSR and download your certificate
6. Double-click the downloaded certificate to install it in Keychain Access
7. Export it as described in the APPLE_CERTIFICATE_BASE64 section above

## Verification

After setting up the secrets, the CI pipeline will:

1. Import the certificate into a temporary keychain
2. Sign both x86_64 and aarch64 macOS binaries with hardened runtime
3. Create zip archives for notarization
4. Submit binaries to Apple's notarization service asynchronously
5. Upload signed binaries to the GitHub release
6. Clean up the temporary keychain

**Note:** Notarization is submitted asynchronously (`--wait false`), so the pipeline won't wait for Apple's approval. You can check notarization status in the Apple Developer portal or by using:

```bash
xcrun notarytool history --apple-id "YOUR_APPLE_ID" --team-id "YOUR_TEAM_ID"
```

## Troubleshooting

### "No identity found" error

- Verify that APPLE_CERTIFICATE_BASE64 is correctly base64-encoded
- Ensure the certificate is a "Developer ID Application" certificate
- Check that APPLE_CERTIFICATE_PASSWORD matches the export password

### "Invalid credentials" during notarization

- Verify APPLE_ID is correct
- Ensure APPLE_APP_PASSWORD is an app-specific password, not your regular password
- Confirm APPLE_TEAM_ID matches your developer account

### Certificate expired

- Check certificate expiration in Apple Developer portal
- Renew certificate and update the APPLE_CERTIFICATE_BASE64 secret

## Security Notes

- Never commit certificates or passwords to version control
- Rotate app-specific passwords periodically
- Use repository secrets (not environment secrets) for better security
- The temporary keychain is automatically cleaned up after signing
