# Releasing YadaCall

The release workflow builds the macOS arm64, Windows x64, and Linux x64 bundles and opens a draft
GitHub Release. Review the draft and publish it when the artifacts and generated notes are ready.
The workflow runs when a `v*` tag is pushed. `workflow_dispatch` builds the version in the checked
out `tauri.conf.json` and creates or updates its `v<version>` draft release.

## Cut a release

1. Make sure the release changes are merged and CI is green.
2. Bump all app version fields together:

   ```sh
   ./scripts/bump-version.sh 0.2.0
   ```

3. Commit the version change, then create and push the matching tag:

   ```sh
   git add Cargo.toml Cargo.lock apps/desktop/package.json apps/desktop/src-tauri/tauri.conf.json apps/desktop/ui/package.json
   git commit -m "Bump version to 0.2.0"
   git tag v0.2.0
   git push origin HEAD
   git push origin v0.2.0
   ```

4. Wait for every platform job to finish. Confirm the draft release contains the `.dmg` and `.app.tar.gz`
   for macOS, NSIS `.exe` and `.msi` installers for Windows, `.AppImage` and `.deb` for Linux, updater
   signatures, and `latest.json`.
5. Review the generated release notes and publish the draft. The updater endpoint serves the latest
   published release, so draft builds are not offered to app users.

Keep the pushed tag, `Cargo.toml` workspace version, Tauri config version, and desktop package versions
in sync. The release action uses the app version to construct the release tag and title.

## GitHub Actions secrets

Add these repository secrets before running the workflow:

| Secret | Purpose |
| --- | --- |
| `TAURI_SIGNING_PRIVATE_KEY` | Private updater signing key; signs the update packages. |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Password used when the updater key was generated. |
| `YADACALL_GOOGLE_CLIENT_ID` | Google OAuth client ID compiled into the desktop app when configured. |
| `YADACALL_GOOGLE_CLIENT_SECRET` | Google OAuth client secret compiled into the desktop app when configured. |

`GITHUB_TOKEN` is provided by GitHub Actions. The release job grants it `contents: write` so the action
can create the draft and upload bundles. Google OAuth secrets may be empty if the integration is not
being released with credentials.

## Generate the updater key

From `apps/desktop`, generate a long-lived key pair and keep the private key in a password manager or
other secure storage:

```sh
pnpm tauri signer generate -w ~/.tauri/yadacall.key
```

Set `TAURI_SIGNING_PRIVATE_KEY` to the contents of `yadacall.key` and
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` to the password used by the command. Put the contents of
`yadacall.key.pub` in `apps/desktop/src-tauri/tauri.conf.json` as `plugins.updater.pubkey`, replacing
`REPLACE_WITH_UPDATER_PUBKEY`. Commit only the public key. Back up the private key: published updater
artifacts must continue to be signed by the same key for existing installations to accept them.

The release action uses the private key to create signed updater artifacts and attaches their
signatures plus `latest.json` to the draft release.

## macOS first launch and signing TODO

The current `bundle.macOS.signingIdentity` value is `-`, an ad-hoc signature. There is no Developer ID
certificate or notarization yet, so Gatekeeper can ask users to approve the downloaded app on first
launch. They can Control-click (right-click) YadaCall in Finder and choose **Open**, then confirm the
prompt. Another workaround after moving it to Applications is:

```sh
xattr -dr com.apple.quarantine /Applications/YadaCall.app
```

When a Developer ID certificate is available, import its `.p12` certificate into CI and configure
`APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, and `KEYCHAIN_PASSWORD`.
Change `bundle.macOS.signingIdentity` from `-` to the Developer ID Application identity. For
notarization, also configure `APPLE_ID`, `APPLE_PASSWORD` (an app-specific password), and
`APPLE_TEAM_ID`, or use App Store Connect API credentials (`APPLE_API_ISSUER`, `APPLE_API_KEY`, and
`APPLE_API_KEY_PATH`). Then enable those environment entries in the macOS release step. Keep the
certificate and notarization credentials in GitHub Secrets.

## Windows code signing TODO

The Windows installers currently have no publisher certificate. When one is available, configure
`WINDOWS_CERTIFICATE` (base64-encoded `.pfx`) and `WINDOWS_CERTIFICATE_PASSWORD` as GitHub Secrets, add
the certificate import/signing step to the Windows job, and configure the Tauri Windows bundle signing
details. The release workflow has marked environment slots for these credentials.
