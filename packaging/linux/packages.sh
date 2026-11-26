#!/usr/bin/env bash
# Builds the .deb and .rpm packages of a Linux release from its tarball, as
# the release workflow does (.github/workflows/release.yml, linux-packages):
#
#   packaging/linux/packages.sh VERSION TARBALL OUT_DIR
#
# TARBALL is helios-VERSION-linux-x86_64.tar.gz (packaging/linux/release.sh);
# its folder is what the packages install in /opt/helios (nfpm.yaml). OUT_DIR
# receives helios_VERSION_amd64.deb, helios-VERSION-1.x86_64.rpm and
# SHA256SUMS-packages. nfpm must be on the PATH (or named by $NFPM).
set -euo pipefail

version="$1"
tarball="$2"
out="$3"
here="$(cd "$(dirname "$0")" && pwd)"
nfpm="${NFPM:-nfpm}"

mkdir -p "$out"
out="$(cd "$out" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

tar -C "$work" -xzf "$tarball"
tree="$(echo "$work"/helios-*-linux-x86_64)"
[ -x "$tree/bin/helios" ] || {
  echo "no bin/helios in $tarball"
  exit 1
}

# The glibc floor: the newest GLIBC_x.y symbol version any program uses
# (the build image's glibc at most), declared as the packages' minimum.
glibc="$(for p in "$tree"/bin/*; do objdump -T "$p"; done |
  grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/^GLIBC_//' | sort -uV | tail -n1)"
[ -n "$glibc" ] || {
  echo "no glibc symbol version found in $tree/bin"
  exit 1
}
echo "glibc floor: $glibc"

sed -e "s|@VERSION@|$version|g" -e "s|@TREE@|$tree|g" -e "s|@GLIBC@|$glibc|g" \
  "$here/nfpm.yaml" >"$work/nfpm.yaml"

deb="helios_${version}_amd64.deb"
rpm="helios-${version}-1.x86_64.rpm"
"$nfpm" package --config "$work/nfpm.yaml" --packager deb --target "$out/$deb"
"$nfpm" package --config "$work/nfpm.yaml" --packager rpm --target "$out/$rpm"

(cd "$out" && sha256sum "$deb" "$rpm" >SHA256SUMS-packages && cat SHA256SUMS-packages)
