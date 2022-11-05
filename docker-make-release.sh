#!/bin/bash

if [ -z "$1" ]; then
   echo "usage: $0 VERSION"
   exit 1
fi

VERSION=$1
SCRIPT_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )

docker run --rm -v"$SCRIPT_DIR:/home/rustacean/src" rustup-android-ndk:v0.1 \
    bash -c "cd src && ./rustup-install-targets.sh && ./make-release.sh $VERSION"
