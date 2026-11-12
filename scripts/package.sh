#!/usr/bin/env bash
# Build a release tarball: dist/hollymic-<version>-<os>-<arch>.tar.gz
# The same file the GitHub release will carry; install.sh installs from it.
#
# Usage: scripts/package.sh [--skip-build]
set -euo pipefail
trap 'printf "\n🔴 FAILED\n" >&2' ERR

cd "$(dirname "$0")/.."

ok()   { printf '🟢 %s\n' "$*"; }
info() { printf '🌱 %s\n' "$*"; }
fail() { printf '🔴 %s\n' "$*" >&2; printf '\n🔴 FAILED\n' >&2; exit 1; }

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)
[[ -n $version ]] || fail "no version in Cargo.toml"
case "$(uname -s)" in
  Linux) os=linux ;;
  Darwin) os=macos ;;
  *) fail "unsupported OS $(uname -s)" ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch=x86_64 ;;
  aarch64 | arm64) arch=aarch64 ;;
  *) fail "unsupported CPU $(uname -m)" ;;
esac
name="hollymic-$version-$os-$arch"

if [[ ${1:-} != --skip-build ]]; then
  info "building release $version (log: target/package-build.log)"
  # Capped so the build can't take the whole laptop (see AGENTS.md).
  if command -v systemd-run >/dev/null && [[ $os == linux ]]; then
    systemd-run --user --scope -q -p MemoryMax=6G cargo build --release --locked -j 4 >target/package-build.log 2>&1 \
      || { tail -n 30 target/package-build.log; fail "cargo build failed"; }
  else
    cargo build --release --locked >target/package-build.log 2>&1 \
      || { tail -n 30 target/package-build.log; fail "cargo build failed"; }
  fi
fi
bin=target/release/hollymic
[[ -x $bin ]] || fail "$bin missing: build first"

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/$name/icons"
cp "$bin" "$stage/$name/hollymic"
"$bin" export-icons "$stage/$name/icons" >/dev/null
rm -f "$stage/$name"/icons/tray-*.png
cp install.sh README.md "$stage/$name/"
[[ -f LICENSE ]] && cp LICENSE "$stage/$name/"
if [[ $os == linux ]]; then
  cp packaging/hollymic.desktop packaging/70-hollymic.rules "$stage/$name/"
fi
echo "$version" >"$stage/$name/VERSION"

mkdir -p dist
tar -C "$stage" -czf "dist/$name.tar.gz" "$name"
(cd dist && sha256sum "$name.tar.gz" >"$name.tar.gz.sha256")
ok "dist/$name.tar.gz ($(du -h "dist/$name.tar.gz" | cut -f1))"
printf '\n🟢 SUCCESS\n'
