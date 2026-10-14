#!/usr/bin/env bash
# Build "Orchard Music Player.app" and dist/Orchard-Music-Player-macOS.dmg.
#
#   packaging/macos/package.sh               # build, bundle, sign, dmg
#   packaging/macos/package.sh --notarize    # ... and notarize + staple the dmg
#
# Options:
#   --identity NAME   code-signing identity (default: $OMP_CODESIGN_IDENTITY, else
#                     the first "Developer ID Application" in the keychain, else ad-hoc)
#   --adhoc           ad-hoc signature (no Developer ID; can't be notarized)
#   --notarize        submit the dmg to Apple's notary service, wait, staple
#   --profile NAME    notarytool keychain profile (default: $OMP_NOTARY_PROFILE or
#                     "orchard-commander", the Orchard family's profile; create one
#                     with `xcrun notarytool store-credentials NAME ...`)
#   --no-build        reuse the last cargo build
#   --dist DIR        output folder (default: dist/ in the repository)
#
# Steps: `cargo build --release` for Apple silicon; the bundle (Info.plist
# from packaging/macos/Info.plist, icon, licenses in Contents/Resources);
# checks that no Mach-O references Homebrew//usr/local or needs a newer
# macOS than MACOSX_DEPLOYMENT_TARGET; hardened-runtime signing with a secure
# timestamp; --self-test of the bundle; the dmg (app + /Applications link).
#
# Output: dist/Orchard Music Player.app, dist/Orchard-Music-Player-macOS.dmg.
# CARGO_TARGET_DIR is honoured.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HERE="$ROOT/packaging/macos"
# Same oldest macOS as Orchard Commander.
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-12.0}"

APP_NAME="Orchard Music Player"
EXE="orchard-music-player"
TARGET=aarch64-apple-darwin
DIST="$ROOT/dist"

identity="${OMP_CODESIGN_IDENTITY:-}"
adhoc=0
notarize=0
build=1
profile="${OMP_NOTARY_PROFILE:-orchard-commander}"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --identity) identity="$2"; shift 2 ;;
    --adhoc) adhoc=1; shift ;;
    --notarize) notarize=1; shift ;;
    --profile) profile="$2"; shift 2 ;;
    --no-build) build=0; shift ;;
    --dist) DIST="$2"; shift 2 ;;
    -h | --help) sed -n '2,/^set -/p' "$0" | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option $1 (see --help)" >&2; exit 2 ;;
  esac
done

mkdir -p "$DIST"
DIST="$(cd "$DIST" && pwd)"
APP="$DIST/$APP_NAME.app"
DMG="$DIST/Orchard-Music-Player-macOS.dmg"

step() { printf '\n==> %s\n' "$*"; }

if [[ $adhoc -eq 1 ]]; then
  identity="-"
elif [[ -z "$identity" ]]; then
  identity="$(security find-identity -v -p codesigning | sed -n 's/.*"\(Developer ID Application:[^"]*\)".*/\1/p' | head -1)"
  if [[ -z "$identity" ]]; then
    echo "No \"Developer ID Application\" identity in the keychain: signing ad-hoc." >&2
    identity="-"
  fi
fi
if [[ $notarize -eq 1 && "$identity" != "Developer ID Application:"* ]]; then
  echo "--notarize needs a \"Developer ID Application\" identity (have: $identity)" >&2
  exit 1
fi

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)"
echo "$APP_NAME $VERSION, macOS $MACOSX_DEPLOYMENT_TARGET+ arm64, signing: $identity"

TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
BIN="$TARGET_DIR/$TARGET/release/$EXE"
if [[ $build -eq 1 ]]; then
  step "cargo build --release"
  (
    unset CFLAGS CPPFLAGS CXXFLAGS LDFLAGS CPATH C_INCLUDE_PATH LIBRARY_PATH PKG_CONFIG_PATH
    cargo build --release --locked --target "$TARGET" --manifest-path "$ROOT/Cargo.toml"
  )
fi
[[ -x "$BIN" ]] || { echo "missing $BIN (build first)" >&2; exit 1; }

step "Bundle $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/licenses"
cp "$BIN" "$APP/Contents/MacOS/$EXE"
sed -e "s/@VERSION@/$VERSION/g" -e "s/@MIN_MACOS@/$MACOSX_DEPLOYMENT_TARGET/g" "$HERE/Info.plist" >"$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist" >/dev/null
printf 'APPL????' >"$APP/Contents/PkgInfo"
cp "$ROOT/packaging/icons/app.icns" "$APP/Contents/Resources/app.icns"
lic="$APP/Contents/Resources/licenses"
cp "$ROOT/LICENSE" "$lic/LICENSE.txt"
if [[ -d "$ROOT/licenses" ]]; then cp -R "$ROOT/licenses/." "$lic/"; fi

step "Check linkage and deployment target"
min_major="${MACOSX_DEPLOYMENT_TARGET%%.*}"
f="$APP/Contents/MacOS/$EXE"
if otool -L "$f" | tail -n +2 | grep -Eq '/opt/homebrew|/usr/local|/Users/'; then
  echo "$f references a non-system path:" >&2
  otool -L "$f" | tail -n +2 | grep -E '/opt/homebrew|/usr/local|/Users/' >&2
  exit 1
fi
minos="$(otool -l "$f" | awk '/LC_BUILD_VERSION/ { b = 1 } b && $1 == "minos" { print $2; exit }')"
if [[ -z "$minos" || "${minos%%.*}" -gt "$min_major" ]]; then
  echo "$f: needs macOS ${minos:-?} (> $MACOSX_DEPLOYMENT_TARGET)" >&2
  exit 1
fi
printf '  %s: macOS %s, %s\n' "$EXE" "$minos" "$(lipo -archs "$f")"

step "Sign"
timestamp="--timestamp"
[[ "$identity" == "-" ]] && timestamp="--timestamp=none"
codesign --force --sign "$identity" --options runtime $timestamp --entitlements "$HERE/entitlements.plist" "$APP"
codesign --verify --deep --strict "$APP"

step "Self-test of the bundle"
scratch="$(mktemp -d "${TMPDIR:-/tmp}/omp-selftest.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT
ditto "$APP" "$scratch/$APP_NAME.app"
ORCHARD_MUSIC_PLAYER_CONFIG_DIR="$scratch/config" "$scratch/$APP_NAME.app/Contents/MacOS/$EXE" --self-test

step "Disk image $DMG"
staging="$scratch/dmg"
mkdir -p "$staging"
ditto "$APP" "$staging/$APP_NAME.app"
ln -s /Applications "$staging/Applications"
rm -f "$DMG"
hdiutil create -volname "$APP_NAME" -srcfolder "$staging" -ov -format UDZO -fs HFS+ "$DMG" >/dev/null
if [[ "$identity" != "-" ]]; then
  codesign --sign "$identity" --timestamp "$DMG"
fi

if [[ $notarize -eq 1 ]]; then
  step "Notarize"
  out="$(xcrun notarytool submit "$DMG" --keychain-profile "$profile" --wait --output-format json)" || true
  status="$(python3 -c 'import json, sys; print(json.loads(sys.argv[1]).get("status", ""))' "$out" 2>/dev/null || true)"
  id="$(python3 -c 'import json, sys; print(json.loads(sys.argv[1]).get("id", ""))' "$out" 2>/dev/null || true)"
  echo "Notarization: ${status:-failed} ${id}"
  if [[ "$status" != "Accepted" ]]; then
    echo "$out" >&2
    [[ -n "$id" ]] && xcrun notarytool log "$id" --keychain-profile "$profile" >&2
    exit 1
  fi
  xcrun stapler staple "$DMG"
  xcrun stapler validate "$DMG"
  spctl --assess --type open --context context:primary-signature -v "$DMG"
fi

echo
echo "App:        $APP"
echo "Disk image: $DMG ($(du -h "$DMG" | cut -f1 | tr -d ' '))"
