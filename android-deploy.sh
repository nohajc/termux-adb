#!/bin/bash

SSH_HOST=u0_a342@192.168.1.156
SSH_PORT=8022

scp -P $SSH_PORT tadb.sh $SSH_HOST:~/termux-adb
scp -P $SSH_PORT target/aarch64-linux-android/release/libadbhooks.so $SSH_HOST:~/termux-adb
