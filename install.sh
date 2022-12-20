#!/data/data/com.termux/files/usr/bin/bash

apt-get update
apt-get  --assume-yes upgrade
apt-get  --assume-yes install coreutils gnupg wget
if [ ! -f "$PREFIX/etc/apt/sources.list.d/termux-adb.list" ]; then
  mkdir -p $PREFIX/etc/apt/sources.list.d
  echo -e "deb https://nohajc.github.io termux extras" > $PREFIX/etc/apt/sources.list.d/termux-adb.list
  wget -qP $PREFIX/etc/apt/trusted.gpg.d https://nohajc.github.io/nohajc.gpg
  apt update
  apt install termux-adb
else
  echo "Repo already installed"
  apt install termux-adb
fi

echo "done!"
