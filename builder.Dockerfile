# syntax=docker/dockerfile:1
# copied from https://github.com/randomnoise/plato-in-container
FROM rust:1.97-slim-trixie AS linaro-files

# Linaro GCC's checksum is same with the Kobo Reader's toolchain Git LFS file reference:
# https://github.com/kobolabs/Kobo-Reader/blob/master/toolchain/gcc-linaro-4.9.4-2017.01-x86_64_arm-linux-gnueabihf.tar.xz
# Alternative Linaro GCC link:
# https://releases.linaro.org/components/toolchain/binaries/4.9-2017.01/arm-linux-gnueabihf/gcc-linaro-4.9.4-2017.01-x86_64_arm-linux-gnueabihf.tar.xz
ADD --checksum=sha256:22914118fd963f953824b58107015c6953b5bbdccbdcf25ad9fd9a2f9f11ac07 \
    https://developer.arm.com/-/cdn-downloads/permalink/legacy-linaro-gnu-toolchains/4.9-2017.01/gcc-linaro-4.9.4-2017.01-x86_64_arm-linux-gnueabihf.tar.xz /

RUN apt-get update \
 && apt-get install --yes --no-install-recommends \
    tar \
    xz-utils \
 && rm --recursive --force /var/lib/apt/lists/* \
### extract Linaro GCC
 && tar --extract --xz --file gcc-linaro-4.9.4-2017.01-x86_64_arm-linux-gnueabihf.tar.xz \
 && mv --verbose /gcc-linaro-4.9.4-2017.01-x86_64_arm-linux-gnueabihf/ /gcc-linaro/ \
 && rm --verbose /gcc-linaro-4.9.4-2017.01-x86_64_arm-linux-gnueabihf.tar.xz

# Keep network downloads in their own layer. This stage only depends on the
# package metadata and download scripts, so source changes do not invalidate it.
FROM rust:1.97-slim-trixie AS plato-downloads

WORKDIR /usr/src/plato

RUN apt-get update \
 && apt-get install --yes --no-install-recommends \
    jq \
    tar \
    unzip \
    wget \
 && rm --recursive --force /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock download.sh ./
COPY crates/core/Cargo.toml crates/core/Cargo.toml
COPY crates/emulator/Cargo.toml crates/emulator/Cargo.toml
COPY crates/fetcher/Cargo.toml crates/fetcher/Cargo.toml
COPY crates/importer/Cargo.toml crates/importer/Cargo.toml
COPY crates/plato/Cargo.toml crates/plato/Cargo.toml
COPY thirdparty/download.sh thirdparty/download.sh

# Cargo validates workspace targets even though this stage only needs the
# package version. Add empty targets so the metadata-only workspace parses.
RUN mkdir --parents \
        crates/core/src \
        crates/emulator/src \
        crates/fetcher/src \
        crates/importer/src \
        crates/plato/src \
 && touch \
        crates/core/src/lib.rs \
        crates/emulator/src/main.rs \
        crates/fetcher/src/main.rs \
        crates/importer/src/main.rs \
        crates/plato/src/main.rs

RUN ./download.sh 'libs/*' \
 && cd thirdparty \
 && ./download.sh mupdf \
 && cd ../libs \
 && ln --symbolic --force libz.so.1 libz.so \
 && ln --symbolic --force libbz2.so.1.0 libbz2.so \
 && ln --symbolic --force libpng16.so.16 libpng16.so \
 && ln --symbolic --force libjpeg.so.9 libjpeg.so \
 && ln --symbolic --force libopenjp2.so.7 libopenjp2.so \
 && ln --symbolic --force libjbig2dec.so.0 libjbig2dec.so \
 && ln --symbolic --force libfreetype.so.6 libfreetype.so \
 && ln --symbolic --force libharfbuzz.so.0 libharfbuzz.so \
 && ln --symbolic --force libgumbo.so.2 libgumbo.so \
 && ln --symbolic --force libdjvulibre.so.21 libdjvulibre.so

FROM rust:1.97-slim-trixie

# add Linaro GCC to $PATH
COPY --from=linaro-files /gcc-linaro/ /gcc-linaro/
ENV PATH=/gcc-linaro/bin:$PATH
# AWS-LC needs a modern assembler, while its objects must use the
# Kobo-compatible Linaro compiler/sysroot to avoid newer glibc symbols.
ENV AWS_LC_SYS_CC=/gcc-linaro/bin/arm-linux-gnueabihf-gcc
ENV AWS_LC_SYS_CXX=/gcc-linaro/bin/arm-linux-gnueabihf-g++
ENV AWS_LC_SYS_CFLAGS="-B/usr/bin/arm-linux-gnueabihf- -march=armv7-a -marm -mfpu=neon -mfloat-abi=hard"

# install dependencies
RUN apt-get update \
 && apt-get install --yes --no-install-recommends \
    git \
    cmake \
    make \
    binutils-arm-linux-gnueabihf \
    jq \
    patch \
    patchelf \
    pkg-config \
    tar \
    unzip \
    wget \
    # xz-utils \
### add armhf as target to rust
 && rustup target add arm-unknown-linux-gnueabihf \
 && rm --recursive --force /var/lib/apt/lists/*

WORKDIR /usr/src/plato

# Pre-populate build.sh's downloaded inputs so it selects its skip path.
COPY --from=plato-downloads /usr/src/plato/libs/ /usr/src/plato/libs/
COPY --from=plato-downloads /usr/src/plato/thirdparty/mupdf/ /usr/src/plato/thirdparty/mupdf/
COPY . .

RUN ./build.sh
