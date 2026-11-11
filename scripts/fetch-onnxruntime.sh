#!/usr/bin/env bash
# Fetches the ONNX Runtime shared library used by the offline VAD chunker.
#
# jifar-audio links ONNX Runtime dynamically (`ort` feature `load-dynamic`) so
# no HTTP crate is ever part of the build (see check-deps.sh). The library is
# only loaded when a recording is chunked after stop; recording never needs it.
#
# Usage: scripts/fetch-onnxruntime.sh [dest-dir]
# Prints the path to libonnxruntime.so; export it as ORT_DYLIB_PATH.
set -euo pipefail

version=1.31.1
sha256=c849e36a1ef3e22d4ff217b9d1ce4a3cf5b9467b975610913ae959ad5b16bdc9
dest=${1:-target/onnxruntime}
name=onnxruntime-linux-x64-$version
lib=$dest/$name/lib/libonnxruntime.so

if [[ ! -f $lib ]]; then
  mkdir -p "$dest"
  tgz=$dest/$name.tgz
  curl -fsSL -o "$tgz" \
    "https://github.com/microsoft/onnxruntime/releases/download/v$version/$name.tgz"
  echo "$sha256  $tgz" | sha256sum -c --quiet -
  tar -xzf "$tgz" -C "$dest"
  rm "$tgz"
fi
realpath "$lib"
