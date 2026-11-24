#!/usr/bin/env bash
# HollyMic installer: install, update or uninstall, for Linux and macOS.
#
#   bash install.sh                     install or update from a release tarball
#   bash install.sh --file <tarball>    use this tarball
#   bash install.sh --uninstall         remove HollyMic (keeps your settings)
#   bash install.sh --uninstall --purge also remove the settings
#   bash install.sh --no-udev           skip the Linux udev rule (no sudo)
#
# Later this runs as a one-liner and fetches the latest GitHub release:
#   curl -fsSL https://raw.githubusercontent.com/rj-xy/hollymic/staging/install.sh | bash
# Until the repository is published it installs a tarball you already have:
# --file, or the newest hollymic-*-<os>-<arch>.tar.gz in this folder or ~/Downloads.
set -euo pipefail
trap 'printf "\n🔴 FAILED\n" >&2' ERR

REPO="rj-xy/hollymic"

ok()   { printf '🟢 %s\n' "$*"; }
info() { printf '🌱 %s\n' "$*"; }
warn() { printf '🟠 %s\n' "$*" >&2; }
fail() { printf '🔴 %s\n' "$*" >&2; printf '\n🔴 FAILED\n' >&2; exit 1; }

mode=install file="" udev=1 purge=0 tmp="" version=""
while [[ $# -gt 0 ]]; do
  case $1 in
    --uninstall) mode=uninstall ;;
    --purge) purge=1 ;;
    --file) file=${2:?--file needs a path}; shift ;;
    --no-udev) udev=0 ;;
    -h | --help) sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) fail "unknown option $1 (see --help)" ;;
  esac
  shift
done

case "$(uname -s)" in
  Linux) os=linux ;;
  Darwin) os=macos ;;
  *) fail "HollyMic runs on Linux and macOS, not $(uname -s)" ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch=x86_64 ;;
  aarch64 | arm64) arch=aarch64 ;;
  *) fail "unsupported CPU $(uname -m)" ;;
esac

bin_dir="$HOME/.local/bin"
data_dir="${XDG_DATA_HOME:-$HOME/.local/share}"
config_dir="${XDG_CONFIG_HOME:-$HOME/.config}"
app_bundle="$HOME/Applications/HollyMic.app"
udev_rule=/etc/udev/rules.d/70-hollymic.rules
bin="$bin_dir/hollymic"

# Stop a running copy so its binary can be replaced and the receiver is released.
stop_running() {
  local was=1
  if [[ -x $bin ]] && "$bin" quit >/dev/null 2>&1; then was=0; fi
  # Wait for every copy to exit (it releases the receiver on the way out); stop
  # any that hang, so two copies never fight over the receiver.
  for _ in $(seq 1 25); do
    pgrep -x hollymic >/dev/null || break
    sleep 0.2
  done
  if pgrep -x hollymic >/dev/null; then
    was=0
    pkill -TERM -x hollymic || true
    sleep 1
    pkill -KILL -x hollymic 2>/dev/null || true
    warn "HollyMic didn't quit by itself; stopped it"
  fi
  [[ $was -eq 0 ]] && info "stopped the running HollyMic"
  return $was
}

sudo_run() {
  if [[ $(id -u) -eq 0 ]]; then "$@"; else sudo "$@"; fi
}

# A rule (ours or a hand-made one) already gives the user the receiver's USB node.
udev_has_rule() {
  grep -hs '^SUBSYSTEM=="usb"' /etc/udev/rules.d/*.rules /usr/lib/udev/rules.d/*.rules 2>/dev/null | grep -q '"3547"'
}

download_latest() {
  # TODO(release): fetch the newest release asset once github.com/$REPO exists:
  #   tag=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p')
  #   curl -fL -o "$tmp/$asset" "https://github.com/$REPO/releases/download/$tag/hollymic-${tag#v}-$os-$arch.tar.gz"
  #   and check it against the .sha256 next to it.
  return 1
}

find_tarball() {
  [[ -n $file ]] && { [[ -f $file ]] || fail "$file not found"; echo "$file"; return; }
  local pattern="hollymic-*-$os-$arch.tar.gz" found=""
  for dir in "$(pwd)" "$(pwd)/dist" "$HOME/Downloads"; do
    found=$(ls -t "$dir"/$pattern 2>/dev/null | head -n1 || true)
    [[ -n $found ]] && { echo "$found"; return; }
  done
  return 1
}

install_linux() {
  local src=$1
  install -Dm755 "$src/hollymic" "$bin"
  sed "s|^Exec=hollymic|Exec=\"$bin\"|" "$src/hollymic.desktop" >"$src/hollymic.desktop.tmp"
  install -Dm644 "$src/hollymic.desktop.tmp" "$data_dir/applications/hollymic.desktop"
  for png in "$src"/icons/hollymic-*.png; do
    size=$(basename "$png" .png); size=${size#hollymic-}
    install -Dm644 "$png" "$data_dir/icons/hicolor/${size}x${size}/apps/hollymic.png"
  done
  command -v update-desktop-database >/dev/null && update-desktop-database -q "$data_dir/applications" 2>/dev/null || true
  command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$data_dir/icons/hicolor" 2>/dev/null || true
  command -v kbuildsycoca6 >/dev/null && kbuildsycoca6 >/dev/null 2>&1 || true
  ok "app menu entry and icons"

  if [[ $udev -eq 1 ]]; then
    if udev_has_rule; then
      ok "udev rule already present"
    else
      info "adding $udev_rule so you can use the receiver without root (asks for your password)"
      if sudo_run install -m644 "$src/70-hollymic.rules" "$udev_rule" \
        && sudo_run udevadm control --reload \
        && sudo_run udevadm trigger --subsystem-match=usb --attr-match=idVendor=3547; then
        ok "udev rule installed"
      else
        warn "couldn't install the udev rule; HollyMic will show how to add it"
      fi
    fi
  else
    warn "skipped the udev rule (--no-udev)"
  fi
}

install_macos() {
  local src=$1
  install -d "$bin_dir"
  install -m755 "$src/hollymic" "$bin"
  # A small app bundle so it's in Launchpad and Spotlight; LSUIElement hides the Dock icon.
  rm -rf "$app_bundle"
  mkdir -p "$app_bundle/Contents/MacOS" "$app_bundle/Contents/Resources"
  cp "$src/hollymic" "$app_bundle/Contents/MacOS/hollymic"
  cat >"$app_bundle/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>HollyMic</string>
  <key>CFBundleIdentifier</key><string>io.github.rj_xy.hollymic</string>
  <key>CFBundleExecutable</key><string>hollymic</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleIconFile</key><string>hollymic</string>
  <key>LSUIElement</key><true/>
</dict></plist>
PLIST
  if command -v iconutil >/dev/null; then
    local set; set=$(mktemp -d)/hollymic.iconset; mkdir -p "$set"
    for s in 16 32 128 256 512; do cp "$src/icons/hollymic-$s.png" "$set/icon_${s}x${s}.png"; done
    for s in 16 128 256; do cp "$src/icons/hollymic-$((s * 2)).png" "$set/icon_${s}x${s}@2x.png" 2>/dev/null || true; done
    iconutil -c icns "$set" -o "$app_bundle/Contents/Resources/hollymic.icns" 2>/dev/null || true
  fi
  xattr -dr com.apple.quarantine "$bin" "$app_bundle" 2>/dev/null || true
  ok "$app_bundle"
}

do_install() {
  local tarball src
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
  if ! tarball=$(find_tarball) && ! tarball=$(download_latest); then
    fail "no hollymic-*-$os-$arch.tar.gz found here or in ~/Downloads; pass --file <tarball> (releases: https://github.com/$REPO/releases)"
  fi
  info "installing from $tarball"
  if [[ -f $tarball.sha256 ]]; then
    (cd "$(dirname "$tarball")" && { sha256sum -c "$(basename "$tarball").sha256" >/dev/null 2>&1 || shasum -a 256 -c "$(basename "$tarball").sha256" >/dev/null 2>&1; }) \
      || fail "checksum mismatch for $tarball"
    ok "checksum"
  fi
  tar -xzf "$tarball" -C "$tmp"
  src=$(find "$tmp" -mindepth 1 -maxdepth 1 -type d | head -n1)
  [[ -x $src/hollymic ]] || fail "the tarball has no hollymic binary"
  version=$(cat "$src/VERSION" 2>/dev/null || echo "?")

  local old="" was_running=0
  [[ -x $bin ]] && old=$("$bin" --version 2>/dev/null | awk '{print $2}')
  stop_running && was_running=1

  if [[ $os == linux ]]; then install_linux "$src"; else install_macos "$src"; fi
  ok "hollymic $version in $bin"

  case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) warn "$bin_dir isn't in your PATH: add it to use the hollymic command" ;;
  esac
  if [[ $was_running -eq 1 ]]; then
    nohup "$bin" >/dev/null 2>&1 &
    ok "restarted HollyMic"
  fi
  if [[ -n $old && $old != "$version" ]]; then
    ok "updated $old → $version"
  elif [[ -n $old ]]; then
    ok "reinstalled $version"
  else
    ok "installed. Start it from the app menu, or run: hollymic"
  fi
  printf '\n🟢 SUCCESS\n'
}

do_uninstall() {
  stop_running || true
  rm -f "$bin"
  if [[ $os == linux ]]; then
    rm -f "$data_dir/applications/hollymic.desktop" "$config_dir/autostart/hollymic.desktop"
    rm -f "$data_dir"/icons/hicolor/*/apps/hollymic.png
    command -v update-desktop-database >/dev/null && update-desktop-database -q "$data_dir/applications" 2>/dev/null || true
    command -v kbuildsycoca6 >/dev/null && kbuildsycoca6 >/dev/null 2>&1 || true
    if [[ -f $udev_rule ]]; then
      if [[ $udev -eq 1 ]]; then
        info "removing $udev_rule (asks for your password)"
        sudo_run rm -f "$udev_rule" && sudo_run udevadm control --reload || warn "couldn't remove $udev_rule"
      else
        warn "left $udev_rule in place (--no-udev)"
      fi
    fi
  else
    rm -rf "$app_bundle"
    rm -f "$HOME/Library/LaunchAgents/io.github.rj_xy.hollymic.plist"
  fi
  if [[ $purge -eq 1 ]]; then
    if [[ $os == linux ]]; then rm -rf "$config_dir/hollymic"; else rm -rf "$HOME/Library/Application Support/hollymic"; fi
    ok "removed settings"
  else
    info "kept your settings (--purge removes them)"
  fi
  ok "HollyMic uninstalled"
  printf '\n🟢 SUCCESS\n'
}

if [[ $mode == uninstall ]]; then do_uninstall; else do_install; fi
