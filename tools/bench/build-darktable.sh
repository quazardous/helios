#!/usr/bin/env bash
# Builds darktable-cli from a darktable checkout and installs it under a
# prefix, once per source commit: a prefix whose stamp names the same
# commit is kept as it is.
#
#   tools/bench/build-darktable.sh <darktable> <rawspeed> <whereami> <libraw> <build> <prefix>
#
# The bench box's mask-parity job runs it (see docs/masks.md, "Parity with
# darktable"). darktable's submodules come as checkouts of their own, at
# the commits darktable pins: rawspeed, whereami and LibRaw are those its
# command line needs (darktable does not build without LibRaw). Everything
# optional is off (no Lua, OpenCL, GUI-only libraries): the build is for
# darktable-cli and its modules.
set -euo pipefail

src=${1:?usage: build-darktable.sh <darktable> <rawspeed> <whereami> <libraw> <build> <prefix>}
rawspeed=${2:?}
whereami=${3:?}
libraw=${4:?}
build=${5:?}
prefix=${6:?}

commit=$(git -C "$src" rev-parse HEAD)
if [ -x "$prefix/bin/darktable-cli" ] && [ "$(cat "$prefix/.commit" 2> /dev/null)" = "$commit" ]; then
  echo "darktable $commit: already built in $prefix"
  exit 0
fi

# a writable copy with its submodules in place, always at the same path so
# the build tree stays valid from one run to the next
tree=$build/src
rm -rf "$tree"
mkdir -p "$tree"
cp -r --preserve=mode "$src"/. "$tree"/
for sub in rawspeed:"$rawspeed" whereami:"$whereami" LibRaw:"$libraw"; do
  rm -rf "$tree/src/external/${sub%%:*}"
  cp -r --preserve=mode "${sub#*:}" "$tree/src/external/${sub%%:*}"
done

# a build tree of another commit starts over
if [ "$(cat "$build/.commit" 2> /dev/null)" != "$commit" ]; then
  rm -rf "$build/tree" "$prefix"
fi
cmake -S "$tree" -B "$build/tree" -G Ninja \
  -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$prefix" \
  -DBINARY_PACKAGE_BUILD=ON -DBUILD_TESTING=OFF \
  -DUSE_LUA=OFF -DUSE_OPENCL=OFF -DUSE_MAP=OFF \
  -DUSE_CAMERA_SUPPORT=OFF -DUSE_COLORD=OFF -DUSE_KWALLET=OFF -DUSE_LIBSECRET=OFF \
  -DUSE_GRAPHICSMAGICK=OFF -DUSE_IMAGEMAGICK=OFF -DUSE_OPENJPEG=OFF -DUSE_JXL=OFF \
  -DUSE_WEBP=OFF -DUSE_AVIF=OFF -DUSE_HEIF=OFF -DUSE_XCF=OFF -DUSE_OPENEXR=OFF \
  -DUSE_PORTMIDI=OFF -DUSE_SDL2=OFF -DUSE_GMIC=OFF -DUSE_ICU=OFF -DUSE_OPENCV=OFF \
  -DUSE_AI=OFF -DUSE_MCP=OFF -DBUILD_PRINT=OFF -DBUILD_CMSTEST=OFF \
  -DBUILD_RS_IDENTIFY=OFF -DUSE_XMLLINT=OFF
echo "$commit" > "$build/.commit"
cmake --build "$build/tree" -j "${DT_JOBS:-$(nproc)}"
cmake --install "$build/tree"
echo "$commit" > "$prefix/.commit"
"$prefix/bin/darktable-cli" --version | head -1
