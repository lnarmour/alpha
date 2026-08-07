#!/bin/bash
# Run as cibuildwheel's `before-all` inside the manylinux container (pyproject.toml's
# [tool.cibuildwheel.linux]) before building the wheel. The container ships neither a Rust
# toolchain nor isl/gmp, and distro packages of isl/gmp (were the image to ship one) tend to ship
# static archives that aren't compiled with -fPIC — linking non-PIC code into the extension
# module's cdylib fails on x86-64 with e.g. "relocation R_X86_64_PC32 cannot be used against symbol
# 'stderr'; recompile with -fPIC" (same failure the VS Code native addon's release build hit and
# documents in more detail, see release.yml). Building both from source with --with-pic avoids
# that; installing to our own prefix and pointing PKG_CONFIG_PATH there (set via cibuildwheel's
# `environment` table, not here — this script's own env doesn't persist to the later build step)
# makes that the copy pkg-config (and thus isl-sys's build.rs) picks up.
set -euo pipefail

dnf install -y clang-devel pkgconfig

curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal

GMP_VERSION=6.3.0
ISL_VERSION=0.26
PREFIX="$HOME/isl-pic"

mkdir -p "$HOME/src" && cd "$HOME/src"

curl -sLO "https://ftp.gnu.org/gnu/gmp/gmp-$GMP_VERSION.tar.xz"
tar xf "gmp-$GMP_VERSION.tar.xz"
cd "gmp-$GMP_VERSION"
./configure --prefix="$PREFIX" --enable-static --disable-shared --with-pic
make -j"$(nproc)"
make install
cd ..

curl -sL "https://libisl.sourceforge.io/isl-$ISL_VERSION.tar.bz2" -o "isl-$ISL_VERSION.tar.bz2"
tar xf "isl-$ISL_VERSION.tar.bz2"
cd "isl-$ISL_VERSION"
./configure --prefix="$PREFIX" --enable-static --disable-shared --with-pic --with-gmp-prefix="$PREFIX"
make -j"$(nproc)"
make install
cd ..
