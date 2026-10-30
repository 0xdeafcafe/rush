# Packaging

`.github/workflows/release.yml` builds rush for macOS (Apple silicon and
Intel) and Linux (x86_64 and arm64). Every night main changed it replaces
the `nightly` prerelease; a `v*` tag makes a release and updates the
Homebrew formula.

- `homebrew/rush.rb`: the formula template; the workflow fills in the
  version and each tarball's sha256 and pushes it to `Formula/rush.rb` in
  [0xdeafcafe/homebrew-tap](https://github.com/0xdeafcafe/homebrew-tap).
- `homebrew/tap-README.md`: the tap's README, pushed alongside it.

## Secrets

Steps whose secrets are missing are skipped: without the Apple ones the
macOS binaries are signed ad-hoc and not notarized; without the deploy key
the tap isn't updated.

| secret | what |
| --- | --- |
| `APPLE_TEAM_ID` | the team of the signing identity (set) |
| `APPLE_CERT_P12` | the "Developer ID Application" certificate and its key, exported as .p12, base64 |
| `APPLE_CERT_PASSWORD` | the .p12's password |
| `APPLE_ID` | the Apple ID that notarizes |
| `APPLE_APP_PASSWORD` | an app-specific password for it (appleid.apple.com) |
| `HOMEBREW_TAP_DEPLOY_KEY` | an ssh private key whose public half is a write deploy key on the tap (set) |

```sh
base64 -i DeveloperIDApplication.p12 | gh secret set APPLE_CERT_P12 -R 0xdeafcafe/rush
gh secret set APPLE_CERT_PASSWORD -R 0xdeafcafe/rush   # prompts for it
gh secret set APPLE_ID -R 0xdeafcafe/rush --body 'you@example.com'
gh secret set APPLE_APP_PASSWORD -R 0xdeafcafe/rush    # prompts for it
```

## Releasing

```sh
git tag v0.2.0 && git push origin v0.2.0
```

The tag must be `v` and the version; rust/Cargo.toml's workspace version
should match it, since nightlies are named after that.
