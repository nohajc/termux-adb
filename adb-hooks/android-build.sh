#!/bin/bash

# arm
export PATH=$PATH:$HOME/Android/android-ndk-r22b/toolchains/llvm/prebuilt/windows-x86_64/bin
#export CC=aarch64-linux-android28-clang
#export CXX=aarch64-linux-android28-clang++

cargo build --release --target aarch64-linux-android
#cargo build --release --target aarch64-unknown-linux-gnu
