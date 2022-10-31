#!/bin/bash

source $(dirname -- "${BASH_SOURCE[0]}")/../deploy-util.sh
scp target/$TARGET_ARCH_TRIPLE/release/termux-fastboot $SSH_TARGET:~/termux-adb
