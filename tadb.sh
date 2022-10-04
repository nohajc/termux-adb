#!/bin/bash

PROC_SHELL="/proc/$PPID"

USB_DEV_FD_FILE="$PROC_SHELL/fd/$TERMUX_USB_FD"
USB_DEV_PATH=$(readlink "$USB_DEV_FD_FILE")
USB_DEV_DIR=$(dirname "$USB_DEV_PATH")

USB_FAKEDEV_PATH=${USB_DEV_PATH/\/dev/.\/fakedev}
USB_FAKEDEV_DIR=${USB_DEV_DIR/\/dev/.\/fakedev}

#echo $USB_FAKEDEV_PATH
#echo $USB_FAKEDEV_DIR

mkdir -p "$USB_FAKEDEV_DIR"
ln -sf "$USB_DEV_FD_FILE" "$USB_FAKEDEV_PATH"

bash -c 'echo $$ && ls -l "'$USB_FAKEDEV_PATH'" && file -L "'$USB_FAKEDEV_PATH'"'

adb kill-server && LD_PRELOAD=./libadbhooks.so adb devices
