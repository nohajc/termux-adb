#!/bin/bash

SCRIPT_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )

function add_targets {
   rustup target add armv7-linux-androideabi
   rustup target add aarch64-linux-android 
}

cd "$SCRIPT_DIR"
pwd
add_targets

cd adb-hooks
pwd
add_targets
