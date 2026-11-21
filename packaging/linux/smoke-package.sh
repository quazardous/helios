#!/usr/bin/env bash
# Smoke test of the .deb or the .rpm (packaging/linux/packages.sh), run by
# the release workflow as root in a clean distribution container (Debian,
# Ubuntu, Fedora, Rocky Linux):
#
#   packaging/linux/smoke-package.sh helios_VERSION_amd64.deb PICTURE.jpg
#   packaging/linux/smoke-package.sh helios-VERSION-1.x86_64.rpm PICTURE.jpg
#
# Installs it with the distribution's own tool (apt or dnf), which proves
# its declared dependencies resolve there; checks the layout (/opt/helios,
# the /usr/bin links, the desktop entry, metainfo and icons) and that every
# library the window's program loads at run time is now present; runs the
# tarball's smoke test (packaging/linux/smoke.sh) on /opt/helios; then
# removes the package and checks none of its files is left behind.
set -euo pipefail

pkg="$1"
picture="$2"
here="$(cd "$(dirname "$0")" && pwd)"
id=io.github.quazardous.Helios
# What helios-gui loads with dlopen (winit, wgpu, egui's GL path); the
# packages declare who provides each (packaging/linux/nfpm.yaml).
loaded="libvulkan.so.1 libEGL.so.1 libwayland-client.so.0 libwayland-egl.so.1
libX11.so.6 libX11-xcb.so.1 libxcb.so.1 libXcursor.so.1 libXi.so.6
libxkbcommon.so.0 libxkbcommon-x11.so.0"

fail() {
  echo "FAIL: $*"
  exit 1
}

echo "== system"
grep PRETTY_NAME /etc/os-release
case "$pkg" in
  *.deb)
    kind=deb
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -q
    echo "== install"
    apt-get install -y -q "$(realpath "$pkg")"
    files() { dpkg -L helios; }
    ;;
  *.rpm)
    kind=rpm
    echo "== install"
    dnf install -y -q "$(realpath "$pkg")"
    files() { rpm -ql helios; }
    ;;
  *) fail "not a .deb or an .rpm: $pkg" ;;
esac
files >/tmp/helios-files.txt
echo "$(wc -l </tmp/helios-files.txt) paths installed"

echo "== layout"
for f in /opt/helios/bin/helios /opt/helios/bin/helios-gui /opt/helios/bin/helios-node \
  /opt/helios/bin/helios-mcp /opt/helios/COPYING /opt/helios/THIRD-PARTY-LICENSES.txt \
  "/usr/share/applications/$id.desktop" "/usr/share/metainfo/$id.metainfo.xml" \
  "/usr/share/icons/hicolor/scalable/apps/$id.svg" \
  "/usr/share/icons/hicolor/256x256/apps/$id.png"; do
  [ -e "$f" ] || fail "$f is missing"
done
for p in helios helios-gui; do
  [ "$(readlink "/usr/bin/$p")" = "/opt/helios/bin/$p" ] || fail "/usr/bin/$p is not a link to /opt/helios/bin/$p"
done
exec_line="$(sed -n 's/^Exec=//p' "/usr/share/applications/$id.desktop")"
command -v "${exec_line%% *}" >/dev/null || fail "the desktop entry's program ${exec_line%% *} is not on the PATH"

echo "== libraries loaded at run time"
missing=""
for so in $loaded; do
  if ldconfig -p | grep -q "^[[:space:]]$so "; then
    echo "$so: $(ldconfig -p | grep -m1 "^[[:space:]]$so " | sed 's/.*=> //')"
  else
    missing="$missing $so"
  fi
done
[ -z "$missing" ] || fail "not installed by the package's dependencies:$missing"

echo "== the programs, from the PATH"
helios --version | tee /tmp/helios-version.txt
grep -q '^helios [0-9]' /tmp/helios-version.txt || fail "helios --version"
helios-gui --version | tee /tmp/helios-gui-version.txt
grep -q '^helios-gui [0-9]' /tmp/helios-gui-version.txt || fail "helios-gui --version"

echo "== the tarball's smoke test on /opt/helios"
bash "$here/smoke.sh" /opt/helios "$picture"

echo "== remove"
if [ "$kind" = deb ]; then
  apt-get purge -y -q helios
  ! dpkg -s helios >/dev/null 2>&1 || fail "helios is still installed"
else
  dnf remove -y -q helios
  ! rpm -q helios >/dev/null 2>&1 || fail "helios is still installed"
fi
left=""
while IFS= read -r f; do
  # Shared folders (/usr/bin, /usr/share/...) stay; anything else goes.
  if [ -L "$f" ] || { [ -e "$f" ] && [ ! -d "$f" ]; }; then left="$left $f"; fi
done </tmp/helios-files.txt
[ ! -e /opt/helios ] || left="$left /opt/helios"
[ -z "$left" ] || fail "left behind:$left"
echo "nothing left behind"
echo "package smoke test passed ($kind)"
