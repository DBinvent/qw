
## Result — qw android signing + profile Qs (Claude Sonnet 5)

- **`qw/app/android-signing-setup.sh`** (new, gitignored via `.gitignore`
  `/app/android-signing-setup.sh`): one-time — `keytool` RSA-4096 PKCS12
  keystore at `~/.android/qw-upload.jks` (never overwrites), writes
  `app/src-tauri/gen/android/keystore.properties`, applies
  `app/android-signing.patch`, prints the CI-secret names. Not run.
- Answers: profile editor UI = not built (`todo-impl.md` §Profile editing,
  `[ ]`). Negotiation = deliberately minimal (nullable offer/counter); the
  trust/earned-tags routing behind it is built, no negotiation UI. No CI
  workflow exists yet.
