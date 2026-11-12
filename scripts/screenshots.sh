#!/usr/bin/env bash
# Regenerate the README screenshots and icon images from the demo receiver.
# Runs the window under Xvfb, so it works while the screen is locked (KDE stops
# painting windows then). Needs: xvfb-run, xdotool, ImageMagick (montage, convert).
set -euo pipefail
trap 'printf "\n🔴 FAILED\n" >&2' ERR
cd "$(dirname "$0")/.."

cargo build -q
bin=target/debug/hollymic
out=docs/images
mkdir -p "$out" assets/icons

shot() {
  env -u WAYLAND_DISPLAY LIBGL_ALWAYS_SOFTWARE=1 timeout 30 \
    xvfb-run -a -s '-screen 0 1024x768x24' "$bin" --demo --screenshot "$@" >/dev/null 2>&1
  printf '🟢 %s\n' "$1"
}
shot "$out/main-dark.png" --theme dark --details
shot "$out/main-light.png" --theme light
shot "$out/settings-dark.png" --theme dark --page settings
shot "$out/settings-light.png" --theme light --page settings

# The phone speaker tooltip: hover its info icon (window is centred on the 1024x768 screen).
cat >"$out/.hover.sh" <<'HOVER'
"$1" --demo --screenshot "$2" --theme dark --details &
pid=$!
sleep 0.9
xdotool mousemove 466 554
sleep 0.2
xdotool mousemove 467 555
wait $pid
HOVER
HOLLYMIC_SHOT_DELAY_MS=3000 env -u WAYLAND_DISPLAY LIBGL_ALWAYS_SOFTWARE=1 timeout 30 \
  xvfb-run -a -s '-screen 0 1024x768x24' bash "$out/.hover.sh" "$PWD/$bin" "$out/phone-speaker-tooltip.png" >/dev/null 2>&1
rm -f "$out/.hover.sh"
printf '🟢 %s\n' "$out/phone-speaker-tooltip.png"

rm -f assets/icons/tray-*.png
"$bin" export-icons assets/icons
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
# One frame per flash phase; steady states use the same picture in both.
states=(tray-no-receiver tray-no-mics tray-nc-on tray-nc-on-both tray-nc-off tray-nc-off-both)
for phase in on off; do
  for bg in dark light; do
    color=$([[ $bg == dark ]] && echo '#232629' || echo '#eff0f1')
    files=()
    for s in "${states[@]}"; do
      if [[ -f assets/icons/$s-$phase.png ]]; then files+=("assets/icons/$s-$phase.png"); else files+=("assets/icons/$s.png"); fi
    done
    montage -background "$color" -geometry 48x48+12+8 -tile 6x1 "${files[@]}" "$tmp/$bg.png"
  done
  montage -background none -geometry +0+0 -tile 1x2 "$tmp/dark.png" "$tmp/light.png" "$tmp/$phase.png"
done
convert -delay 50 -loop 0 "$tmp/on.png" "$tmp/off.png" -layers Optimize "$out/tray-states.gif"
rm -f "$out/tray-states.png"
printf '🟢 %s\n' "$out/tray-states.gif"
printf '\n🟢 SUCCESS\n'
