# Releasing Orchard Music Player

All packages come from CI (`.github/workflows/ci.yml`), the macOS dmg
included: built, signed with the Developer ID and notarized there.

## Every push and pull request

- **Tests** (ubuntu-22.04, windows-latest, macos-latest): clippy with
  `-D warnings`, `cargo test`, a release build and its `--self-test`.
- **Package** (windows-latest, ubuntu-22.04): the app folder
  (`packaging/stage.sh`: the executable plus `licenses/`), its `--self-test`,
  then
  - Windows: `Orchard-Music-Player-Setup.exe` (Inno Setup,
    `packaging/windows/orchard-music-player.iss`; keep its AppId) and
    `Orchard-Music-Player-Windows.zip`, tested with a silent install,
    self-test and uninstall.
  - Linux: `Orchard-Music-Player-Linux.tar.gz`,
    `Orchard-Music-Player-Linux-x86_64.{deb,rpm}` (nfpm,
    `packaging/linux/nfpm.yaml`) and
    `Orchard-Music-Player-Linux-x86_64.AppImage`, tested with apt (the runner
    and clean Ubuntu/Debian containers), dnf (Fedora container) and by
    running the AppImage and the unpacked tarball.
  - The files are the workflow artifacts `packages-Windows` and
    `packages-Linux`.
- **Package** (macos-latest): `Orchard-Music-Player-macOS.dmg` from
  `packaging/macos/package.sh --notarize` (Apple silicon, macOS 12+), then
  the app copied out of the dmg and started once (first frame, quit). The
  workflow artifact is `packages-macOS`.

## macOS signing secrets

The macOS job imports these repository secrets into a temporary keychain,
made the default for the job, under the notary profile name
`orchard-commander`, so `package.sh` runs exactly as on a Mac:

| Secret | Contents |
|---|---|
| `MACOS_CERTIFICATE_P12_BASE64` | the "Developer ID Application" certificate and its private key, exported as `.p12`, base64 |
| `MACOS_CERTIFICATE_PASSWORD` | the `.p12`'s password |
| `APPLE_API_KEY_P8` | the App Store Connect API key (`AuthKey_….p8`, the whole text) |
| `APPLE_API_KEY_ID` | that key's ID |
| `APPLE_API_ISSUER_ID` | the issuer ID (App Store Connect → Users and Access → Integrations) |

Without them (pull requests from forks) the dmg is signed ad-hoc and not
notarized; a tag fails without them. `packaging/macos/package.sh` still
builds the dmg on a Mac (`--notarize` with a local keychain profile).

## Licenses

`licenses/` ships in every package (`licenses\` in the Windows install
folder, `/opt/orchard-music-player/licenses` and
`/usr/share/doc/orchard-music-player` on Linux): our `LICENSE`, libopus's
BSD license (`libopus.txt`) and `THIRD-PARTY-RUST.txt`, generated from
`Cargo.lock` by `scripts/third-party-licenses.sh` (needs cargo-about). Rerun
it after changing dependencies or the version and commit the result; CI
warns when it is stale and regenerates it for the packages. The macOS script
gets the same folder with `packaging/stage.sh licenses - <dir>`.

## Making a release

1. Bump `version` in `Cargo.toml`, run `cargo check` (so `Cargo.lock`
   follows) and `scripts/third-party-licenses.sh`; commit and push to main
   and wait for a green CI run.
2. Tag and push: `git tag v1.2.3 && git push github v1.2.3`. CI checks that
   the tag matches `Cargo.toml`, creates the draft release "Orchard Music
   Player 1.2.3" if it doesn't exist and uploads the Windows, Linux
   and macOS packages to it (`gh release upload --clobber`, so a rerun
   replaces them). CI never publishes.
3. Write the release notes and publish the release on GitHub.
4. Update the Homebrew cask in `sandboxly/homebrew-tap` (version and the
   dmg's SHA-256).
