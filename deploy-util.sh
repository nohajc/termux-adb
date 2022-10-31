#!/bin/bash

if [[ -z $1 || -z $2 ]]; then
   echo "usage: $0 SSH_TARGET armv7l|aarch64"
   exit 1
fi

SSH_TARGET="$1"
TARGET_ARCH="$2"
TARGET_ARCH_TRIPLE="$TARGET_ARCH"

if [[ "$TARGET_ARCH" == "armv7" ]]; then
    TARGET_ARCH_TRIPLE="armv7-linux-androideabi"
elif [[ "$TARGET_ARCH" == "aarch64" ]]; then
    TARGET_ARCH_TRIPLE="aarch64-linux-android"
fi
