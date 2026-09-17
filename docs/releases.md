# Spotidark releases and automatic updates

Spotidark publishes two downloads:

- `spotidark-macos.dmg`: a universal app for Apple silicon and Intel Macs.
- `spotidark-windows.exe`: a Windows x64 installer, installed for the current user.

The GitHub release page also provides GitHub's automatic source archive links.
The workflow does not upload ZIPs, tarballs, Linux packages, duplicate legacy
binaries, checksum files, or appcasts. Linux users can build from source.

## Automatic delivery

Every push to `main` runs CI. After quality, all platform tests, Nix, and docs
pass, CI directly calls the release workflow to build and publish both installers.
It builds that exact commit and checks that it is still current before
publication. Failed or superseded builds cannot replace the current release.

Releases roll forward: the new release is published and verified before older
published version releases are removed. Tags remain as source-history references.
Drafts and unrelated releases are preserved. Download the current installers
from [the latest release](https://github.com/darkroomengineering/spotidark/releases/latest).
Each build retains its own increasing version and tag, which the existing OTA
client understands. An older download can fail when its release is removed;
checking for updates again selects the new version. Asset bytes are never
replaced under the same version.

Installers upload to a draft first. Their GitHub sizes and digests must match
the local files, and main must still identify the tested commit, before the
draft becomes public. Failed publication leaves the draft private for inspection
and preserves the previous public release. If removal of older releases fails,
the job reports the error and retains the new valid release.

Release versions retain the source major and minor version and use an increasing
patch number derived from the CI workflow run. The build stamps the version
into its temporary Cargo manifest and lockfile without modifying the source
branch. Manually running **CI** on main follows the same checks and publication
path. Manually running **Release desktop apps** builds review artifacts only.

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

Apple signing setup is currently deferred. Releases without complete signing
credentials use an ad-hoc signature and are not notarized; macOS may block their
first launch. Release notes state the signing mode. Windows installers are not
Authenticode signed.

To enable Developer ID signing, notarization, and stapling for both the app and
DMG, configure these GitHub Actions secrets using the same Apple account and
Developer ID as Programa:

- `APPLE_CERTIFICATE_BASE64`
- `APPLE_CERTIFICATE_PASSWORD`
- `APPLE_SIGNING_IDENTITY`
- `APPLE_ID`
- `APPLE_APP_SPECIFIC_PASSWORD`
- `APPLE_TEAM_ID`

Keep their values in the secret store, never the repository. Spotidark does not
need Programa's Sparkle key or CloudKit provisioning profile. Once complete
credentials are present, signing or notarization errors stop publication; they
never fall back to publishing an ad-hoc build.

The inherited Homebrew, AUR, Flatpak, and native Linux packaging configuration
remains upstream reference material and is not part of Spotidark publication.
Spotifast's MIT license and contributor credits remain included in the app and
installer.
