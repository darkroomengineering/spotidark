# Spotidark releases and automatic updates

Spotidark publishes two downloads:

- `spotidark-macos.dmg`: a universal app for Apple silicon and Intel Macs.
- `spotidark-windows.exe`: a Windows x64 installer, installed for the current user.

The GitHub release page also provides GitHub's automatic source archive links.
The workflow does not upload ZIPs, tarballs, Linux packages, duplicate legacy
binaries, checksum files, or appcasts. Linux users can build from source.

## Automatic delivery

A successful CI run for the current `main` commit triggers the release workflow.
It builds that exact commit, checks that it is still current before publication,
and publishes only after both platform builds succeed. Each release has its own
version and tag; update downloads refer to that tag, so later releases cannot
replace bytes while an older download is in progress. Installers upload to a
draft first. Their GitHub sizes and digests must match the local files, and
main must still identify the tested commit, before the draft becomes public.
Failed publication leaves the draft private for inspection. A new successful
CI run creates a new version; retries never overwrite an existing release.

Release versions retain the source major and minor version and use an increasing
patch number derived from the release workflow run. The build stamps the version
into its temporary Cargo manifest and lockfile without modifying the source
branch. Manual workflow dispatch builds review artifacts without publishing.

The installed app checks on startup and once an hour. Background downloads are on
by default; explicit saved opt-outs are preserved. **Restart to update** applies
the downloaded update when the listener is ready. Music is not interrupted by
a background check or download. Settings offers a manual check and update
preferences.

The updater obtains the release asset's SHA-256 digest from GitHub's
[release metadata](https://docs.github.com/en/rest/releases/assets), verifies
the download size and digest, and stages the update beside the installation.
Missing or invalid digests stop the update. These are GitHub-hosted integrity
checks, not an independent publisher signature. No appcast is required by
Spotidark's native updater.

macOS checks the bundle identifier, version, and code signature. Developer ID
installations also require the same signing team and a passing Gatekeeper
assessment. Move Spotidark out of the DMG into Applications before updating.
Windows installs into the user's Programs/Spotidark folder and uses its own
installer and registry identity, separate from Spotifast. Windows downloads
have SHA-256 verification; they are not described as Authenticode signed.
Settings, caches, and Spotify credentials survive updates.

Portable, source-built, and package-manager installations do not receive the
DMG/EXE installer updates. Rebuild or use the corresponding package manager.

## Signing configuration

Public macOS releases must be Developer ID signed, notarized, and stapled. The
app and its DMG are checked before publication. Configure these GitHub Actions
secrets using the same Apple account and Developer ID as Programa:

- `APPLE_CERTIFICATE_BASE64`
- `APPLE_CERTIFICATE_PASSWORD`
- `APPLE_SIGNING_IDENTITY`
- `APPLE_ID`
- `APPLE_APP_SPECIFIC_PASSWORD`
- `APPLE_TEAM_ID`

Keep their values in the secret store, never the repository. Spotidark does not
need Programa's Sparkle key or CloudKit provisioning profile. Missing signing
credentials must prevent a public release; build-only artifacts are not proof
of a signed, notarized release.

The inherited Homebrew, AUR, Flatpak, and native Linux packaging configuration
remains upstream reference material and is not part of Spotidark publication.
Spotifast's MIT license and contributor credits remain included in the app and
installer.
