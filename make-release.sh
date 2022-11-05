#!/bin/bash

if [ -z "$1" ]; then
   echo "usage: $0 VERSION"
   exit 1
fi

VERSION=$1

set -e

SCRIPT_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )

cd "$SCRIPT_DIR"

# Build
(cd adb-hooks && ./android-build.sh)
(cd termux-adb && ./android-build.sh)
(cd termux-fastboot && ./android-build.sh)

# Archive
mkdir -p dist
cd dist

mkdir "$VERSION"
cd "$VERSION"

function archive_release {
   RELEASE_DIR=$1
   TARGET_TRIPLE=$2

   mkdir "$RELEASE_DIR"
   cd "$RELEASE_DIR"
   cp "$SCRIPT_DIR/termux-adb/target/$TARGET_TRIPLE/release/termux-adb" .
   cp "$SCRIPT_DIR/adb-hooks/target/$TARGET_TRIPLE/release/libadbhooks.so" .
   cp "$SCRIPT_DIR/termux-fastboot/target/$TARGET_TRIPLE/release/termux-fastboot" .
   cd ..
   tar cvpzf "$RELEASE_DIR.tar.gz" "$RELEASE_DIR"
   rm -r "$RELEASE_DIR"
}

archive_release "termux-adb-$VERSION-aarch64" "aarch64-linux-android"
archive_release "termux-adb-$VERSION-armv7" "armv7-linux-androideabi"
