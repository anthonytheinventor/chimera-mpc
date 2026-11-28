#!/bin/bash
# train.sh <instrumented engine .a> <profiler_builtins .rlib> <Content folder> <out .profraw>
#
# Links tools/pgo/train.c with an instrumented arm32 engine and runs it under qemu-arm, which writes
# the raw profile. Called by tools/pgo_profile.py (in WSL on Windows). Needs arm-linux-gnueabihf-gcc
# with its libc (Ubuntu: gcc-arm-linux-gnueabihf) and qemu-arm (qemu-user).
set -euo pipefail
LIB=${1:?instrumented libchimera_mpc_engine.a}
RLIB=${2:?libprofiler_builtins .rlib}
CONTENT=${3:?Content folder}
OUT=${4:?output .profraw}
HERE=$(cd "$(dirname "$0")" && pwd)
W=$(mktemp -d)
trap 'rm -rf "$W"' EXIT
# the profiler runtime: rustc does not bundle it into a staticlib, so its objects are linked here
mkdir "$W/prof"
(cd "$W/prof" && arm-linux-gnueabihf-ar x "$RLIB" && rm -f lib.rmeta* && arm-linux-gnueabihf-ar rcs ../libprof.a ./*.o)
arm-linux-gnueabihf-gcc -O2 -mcpu=cortex-a17 -mfpu=neon-vfpv4 -mfloat-abi=hard -o "$W/train" "$HERE/train.c" \
    "$LIB" "$W/libprof.a" -Wl,-u,__llvm_profile_runtime -lpthread -ldl -lm -lrt -lutil
LLVM_PROFILE_FILE="$W/train.profraw" qemu-arm -L /usr/arm-linux-gnueabihf "$W/train" "$CONTENT"
cp "$W/train.profraw" "$OUT"
