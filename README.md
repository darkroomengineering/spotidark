# Spotidark

A native Spotify client by **Darkroom Engineering**, built on
[Spotifast](https://github.com/crmne/spotifast) by **Carmine Paolino and the
Spotifast contributors**. Their work provides the application, playback engine
integration, and desktop features that make this fork possible.

Spotidark keeps the Rust, egui, and librespot architecture, with a monochrome
Darkroom interface and an independent app identity. Spotify remains the music
source. **Local playback requires Spotify Premium.**

## What you can do

- Play music locally and control Spotify Connect devices.
- Browse your library, playlists, albums, artists, podcasts, and saved episodes.
- Search, manage playlists, save songs, and use the queue and lyrics views.
- Use media keys, keyboard shortcuts, the system tray, and Spotify links.
- Use light, dark, and custom themes, the equalizer, Winamp skins, and MilkDrop.

These capabilities are inherited from Spotifast. Spotify account permissions,
API availability, and librespot support still determine what works.

## Build and run

This fork currently builds from source. Spotifast's Homebrew, AUR, Flatpak,
and other published packages install the upstream app, **not Spotidark**.

```sh
git clone https://github.com/darkroomengineering/spotidark.git
cd spotidark
cargo run --locked --release --bin spotidark
```

Rust is pinned in `rust-toolchain.toml`. MilkDrop also needs CMake, a C++
compiler, and libclang. On Linux, install the ALSA, PulseAudio, xkbcommon,
and Wayland development libraries. See the preserved
[upstream build documentation](https://github.com/crmne/spotifast#install)
for platform prerequisites. A smaller build without MilkDrop is available:

```sh
cargo run --locked --release --no-default-features --bin spotidark
```

Install only the fork command so upstream commands can coexist:

```sh
cargo install --path . --locked --bin spotidark
```

The internal Rust package and library retain the name `fastpotify` to keep
upstream integration straightforward. The compatibility binaries `spotifast`
and `fastpotify` also build, but are not needed to run Spotidark.

On macOS, build an app bundle after compiling the release binary:

```sh
bash packaging/macos/bundle.sh target/release/spotidark /tmp/Spotidark.app 0.8.0
open /tmp/Spotidark.app
```

The script uses an ad-hoc signature unless `CODESIGN_IDENTITY` is supplied.
This is a local development bundle, not a notarized public release.
Linux launcher assets are `packaging/applications/spotidark.desktop` and
`packaging/icons/spotidark.svg`; the `spotidark` command must be on your PATH.

## Sign in and Spotify access

Choose **Sign in with Spotify**. Approval happens in your browser on Spotify's
own pages. Spotidark retains upstream's shared Web API access and separate
librespot playback authorization. Add a personal Spotify developer app in
**Settings → Account** to use its separate quota for supported requests.

Darkroom owns this repository and the fork's release destination. It does not
own the inherited public shared Spotify client registration or Spotify's
service. No Darkroom Spotify developer registration is bundled in this first
version. See [how the inherited connections work](docs/_reference/how-it-connects.md)
and [supported Spotify capabilities](docs/_reference/what-spotify-allows.md).

Spotidark uses separate settings, caches, credential storage, and desktop
instance identifiers. Sign in again when switching from Spotifast; the fork
does not import or clear Spotifast's credentials. On Linux, settings are in
`~/.config/spotidark/settings.json`. On macOS and Windows, they use the native
application directories for the `engineering` / `darkroom` / `spotidark` identity.

Update checks point to
[Darkroom's releases](https://github.com/darkroomengineering/spotidark/releases).
There is no Spotidark binary release yet. The inherited publishing workflows
are gated off in this fork because their package identities and destinations
still describe Spotifast. Build from source until fork packages are published.

## Personal Spotify app

Follow the [personal Spotify app guide](docs/spotify-app.md) to create your own
app and connect it in Settings. The app includes all five setup steps, a button
to copy the exact redirect URI, and Client ID validation before authorization.
Each developer needs an app under their own developer account: Development Mode
allows 5 allow-listed users, and quota is counted per developer account.

## Development and upstream updates

```sh
git remote add upstream https://github.com/crmne/spotifast.git
git fetch upstream
cargo run --locked --features demo --bin spotidark -- --demo
```

Demo mode uses sample data without signing into Spotify. See
[CONTRIBUTING.md](CONTRIBUTING.md) for checks and product boundaries.
Keep upstream changes reviewable, preserve contributor history, and retain
copyright notices when integrating updates.

The detailed documentation and legacy packaging recipes remain as upstream
reference material. Their Spotifast download links, migration instructions,
and release history do not describe Spotidark distributions.

## Credits and license

- **[Carmine Paolino](https://github.com/crmne) and the
  [Spotifast contributors](https://github.com/crmne/spotifast/graphs/contributors):**
  original application and ongoing upstream development.
- **[librespot](https://github.com/librespot-org/librespot)** and
  **[egui](https://github.com/emilk/egui):** Spotify playback and native interface.
- **[Inter](https://rsms.me/inter/)** (OFL), **[Lucide](https://lucide.dev)** (ISC),
  and **[projectM](https://github.com/projectM-visualizer/projectm):** typography,
  icons, and MilkDrop visualizations.

The original [MIT license and copyright notice](LICENSE) are preserved.
Spotidark is an independent fork, not affiliated with or endorsed by Spotify AB
or the Spotifast team. Spotify is a trademark of Spotify AB.
