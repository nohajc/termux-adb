#!/bin/bash

SCRIPT_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )
source "$SCRIPT_DIR/../deploy-util.sh"
scp target/$TARGET_ARCH_TRIPLE/release/termux-adb $SSH_TARGET:~/termux-adb
