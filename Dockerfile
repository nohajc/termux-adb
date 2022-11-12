FROM ubuntu:22.04

RUN apt-get update && apt-get -y upgrade && apt-get -y install curl ca-certificates build-essential unzip libssl-dev

RUN useradd -s /bin/bash -m rustacean
USER rustacean

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o /tmp/rustup-init.sh \
	&& sh /tmp/rustup-init.sh -y

RUN cd $HOME \
    && curl https://dl.google.com/android/repository/android-ndk-r22b-linux-x86_64.zip -o android-ndk.zip \
	&& unzip android-ndk.zip \
	&& rm android-ndk.zip \
	&& echo "[target.armv7-linux-androideabi]" > .cargo/config \
	&& echo "linker = \"$HOME/android-ndk-r22b/toolchains/llvm/prebuilt/linux-x86_64/bin/armv7a-linux-androideabi21-clang\"" >> .cargo/config \
	&& echo "" >> .cargo/config \
	&& echo "[target.aarch64-linux-android]" >> .cargo/config \
	&& echo "linker = \"$HOME/android-ndk-r22b/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android21-clang\"" >> .cargo/config

WORKDIR /home/rustacean
RUN mkdir /home/rustacean/src
ENV PATH "/home/rustacean/.cargo/bin:$PATH"
