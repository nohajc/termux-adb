# termux-adb

Run adb in Termux without root permissions!

## Description

This is a launcher for adb which enables debugging of one Android device from another via USB cable.
It should work with any USB-C male-to-male cable or the corresponding OTG adapter + cable in case of micro USB.

## Installation

- install Termux from F-Droid
- install Termux:API from F-Droid
- in Termux:
```
$ pkg install termux-api android-tools
```

- download binaries for your target architecture from [Releases](https://github.com/nohajc/termux-adb/releases)
- copy `termux-adb` and `libadbhooks.so` to some path accessible from Termux session
  (both files must be in the same directory)

## Usage

First start adb server using the special launcher
```
$ ./termux-adb
```

Then you can run any adb commands directly, e.g.
```
$ adb devices
```

## Build instructions

(You can check the Dockerfile for minimal toolchain setup)

Requirements: linux environment with working gcc and [rustup](https://rustup.rs/) plus all of the following:

1. download [Android NDK r22b](https://github.com/android/ndk/wiki/Unsupported-Downloads#r22b) (I wasn't able to make it work with any newer NDK version)
2. add linker paths to ~/.cargo/config

```
[target.armv7-linux-androideabi]
linker = "/abs/path/to/ndk/toolchains/llvm/prebuilt/linux-x86_64/bin/armv7a-linux-androideabi21-clang"

[target.aarch64-linux-android]
linker = "/abs/path/to/ndk/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android21-clang"
```

3. install cross-compilation targets (this will also install rust nightly for the `c_variadic` feature)
`$ ./rustup-install-targets.sh`

4. build individual components (termux-adb, adb-hooks, termux-fastboot) using `./android-build.sh`
5. alternatively run `./make-release.sh` to build and package all of them at once

There are also scripts for creating a docker toolchain image (`./docker-make-toolchain.sh`) and building the project inside it (`./docker-make-release.sh`)

## How it actually works

Termux has the `android-tools` package which contains `adb` (and `fastboot`) but it normally works on rooted devices only.
This is mainly due to filesystem permissions required by adb when enumerating USB devices (traversing `/dev/bus/usb/*`).

There is, however, Android API exposed by `termux-usb` utility which gives you a raw file descriptor of any connected USB device after manual approval by the user.

Of course, `adb` by itself doesn't know anything about `termux-usb` nor it can take raw file descriptors from command-line or environment.
If it cannot access `/dev/bus/usb`, it just won't detect any connected devices. This is where `termux-adb` comes in.

To avoid the need for patching `adb` source code, `termux-adb` uses `LD_PRELOAD` to inject a dynamic library that hooks a couple of libc functions and emulates access to `/dev/bus/usb` as if the corresponding directory structure was accessible. There it will set up a virtual character device backed by the file descriptor obtained from `termux-usb`.

Because `adb` forks itself and runs in the background when you run it for the first time, it means it can scan for newly connected USB devices continuously.
In order to emulate this behavior, `termux-adb` also runs in the background, polls `termux-usb` periodically and sends any discovered file descriptors via a Unix Domain Socket connection to the injected library running along `adb` server. That way the virtual directory tree is kept up to date which is reflected in the output of `adb devices`.

Currently, `termux-adb` is limited to one USB device at a time but this can be improved in the future.
