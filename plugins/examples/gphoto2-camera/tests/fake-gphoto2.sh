#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# A fake gphoto2 for tests: one camera on usb:001,004, whose files are kept
# under $FAKE_GPHOTO2_DIR. It answers the few commands the plugin runs, in
# gphoto2's own words.
set -e
state="${FAKE_GPHOTO2_DIR:?FAKE_GPHOTO2_DIR is not set}"
folder=/store_00010001/DCIM/100HELIO
port=""
filename=""
action=""
number=""
while [ $# -gt 0 ]; do
    case "$1" in
        --port) port="$2"; shift 2 ;;
        --filename) filename="$2"; shift 2 ;;
        --get-file) action="get"; number="$2"; shift 2 ;;
        --auto-detect|--summary|--capture-image|--list-files) action="$1"; shift ;;
        *) echo "fake gphoto2: unknown argument $1" >&2; exit 2 ;;
    esac
done
if [ -n "$port" ] && [ "$port" != "usb:001,004" ]; then
    echo "*** Error: no camera on $port" >&2
    exit 1
fi
mkdir -p "$state"
# The pictures on the camera, in name order: IMG_0001.JPG, IMG_0002.JPG...
pictures() {
    for path in "$state"/*; do
        if [ -e "$path" ]; then basename "$path"; fi
    done
}
case "$action" in
    --auto-detect)
        echo "Model                          Port"
        echo "----------------------------------------------------------"
        echo "Helios Fake Camera             usb:001,004"
        ;;
    --summary)
        echo "Camera summary:"
        echo "Manufacturer: Helios"
        echo "Model: Fake Camera"
        echo "  Version: 1.0"
        echo "  Serial Number: 0042"
        ;;
    --capture-image)
        n=$(($(pictures | wc -l) + 1))
        name=$(printf 'IMG_%04d.JPG' "$n")
        echo "fake picture $n" > "$state/$name"
        echo "New file is in location $folder/$name on the camera"
        ;;
    --list-files)
        echo "There is no file in folder '/'."
        echo "There are $(pictures | wc -l) files in folder '$folder':"
        i=0
        for f in $(pictures); do
            i=$((i + 1))
            printf '#%-5d %-27s rd     1 KB image/jpeg\n' "$i" "$f"
        done
        ;;
    get)
        f=$(pictures | sed -n "${number}p")
        if [ -z "$f" ]; then
            echo "*** Error: no file number $number" >&2
            exit 1
        fi
        cp "$state/$f" "${filename:-$f}"
        echo "Saving file as ${filename:-$f}"
        ;;
    *)
        echo "fake gphoto2: nothing to do" >&2
        exit 2
        ;;
esac
