# termux-adb

Run adb and fastboot in Termux without root permissions!

## Description

This is a modified version of adb and fastboot which enables debugging of one Android device from another via USB cable.
It should work with any USB-C male-to-male cable or the corresponding OTG adapter + cable in case of micro USB.

## Installation

- install Termux from F-Droid
- install Termux:API from F-Droid
- in Termux:
```
$ curl -s https://raw.githubusercontent.com/nohajc/termux-adb/master/install.sh | bash
```

- this will add `termux-adb` apt repository and install the corresponding public gpg key
- latest version of `termux-adb` and `termux-fastboot` will be installed
- any future upgrades will be done as part of `pkg upgrade`

## Usage

Both `termux-adb` and `termux-fastboot` are drop-in replacements for the original commands so the usage is exactly the same.
The commands were only renamed to avoid collision with the official `android-tools` Termux package (which contains more tools beside these two).

## Build instructions

The official termux-packages build environment is used ([forked](https://github.com/nohajc/termux-packages) to add the `termux-adb` package).

For more information, please refer to the Termux documentation:
- https://github.com/termux/termux-packages/wiki/Build-environment
- https://github.com/termux/termux-packages/wiki/Building-packages

## Current limitations

Using `termux-usb` and querying device serial number with `libusb` tends to be slow. That's not a problem for adb which runs as a daemon and scans USB devices periodically. However, it is quite noticable for `termux-fastboot` commands because fastboot doesn't use any background service. This can potentially be improved in a future release.

## How it actually works

Termux has the `android-tools` package which contains `adb` and `fastboot` but it normally works on rooted devices only.
This is mainly due to filesystem permissions required by adb when enumerating USB devices (traversing `/dev/bus/usb/*`).

There is, however, Android API exposed by `termux-usb` utility which gives you a raw file descriptor of any connected USB device after manual approval by the user.

Of course, `adb` by itself doesn't know anything about `termux-usb` nor it can take raw file descriptors from command-line or environment.
If it cannot access `/dev/bus/usb`, it just won't detect any connected devices. This is where `termux-adb` comes in.

Both `adb` and `fastboot` are patched to scan for USB devices using the `termux-usb` command. Furhtermore a Unix Domain Socket is used to transfer the obtained file descriptors from child process to the parent (i.e. `termux-adb` runs `termux-usb` for every detected device which in turn runs `termux-adb` in a special mode that will only send USB file descriptor to the UDS file descriptor provided by environment variable).

This way we don't complicate the user experience and we can work with any number of devices connected at once (e.g. if you have a USB hub connected to the OTG adapter).
