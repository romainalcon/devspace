#!/bin/sh
# Builds a Valkey release from its official sources into a DevSpace package:
#   valkey-<version>-<platform>.tar.gz  with  valkey-<version>/{bin/valkey-server,bin/valkey-cli,COPYING}
#
# usage: build-valkey.sh <version> <platform> <output-dir>
#   <platform>: macos-aarch64, macos-x86_64, linux-x86_64 or linux-aarch64
#
# The source archive must match the SHA-256 pinned in valkey-sources.txt.
# Built without TLS and without systemd: the programs only need the C
# library (glibc 2.28 when built in the manylinux_2_28 image, macOS 12).
set -eu

version=$1
platform=$2
out=$(mkdir -p "$3" && cd "$3" && pwd)
here=$(cd "$(dirname "$0")" && pwd)

expected=$(awk -v v="$version" '$1 == v { print $2 }' "$here/valkey-sources.txt")
if [ -z "$expected" ]; then
  echo "valkey $version is not pinned in valkey-sources.txt" >&2
  exit 1
fi

case "$platform" in
  macos-*) export MACOSX_DEPLOYMENT_TARGET=12.0; sha() { shasum -a 256 "$1" | cut -d' ' -f1; } ;;
  linux-*) sha() { sha256sum "$1" | cut -d' ' -f1; } ;;
  *) echo "unknown platform $platform" >&2; exit 1 ;;
esac

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cd "$work"
curl --proto '=https' --tlsv1.2 -sSfL -o src.tar.gz \
  "https://github.com/valkey-io/valkey/archive/refs/tags/$version.tar.gz"
actual=$(sha src.tar.gz)
if [ "$actual" != "$expected" ]; then
  echo "source checksum mismatch: $actual, expected $expected" >&2
  exit 1
fi
tar xzf src.tar.gz
cd "valkey-$version"
jobs=$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 2)
make -j"$jobs" BUILD_TLS=no USE_SYSTEMD=no

pkg="$work/package/valkey-$version"
mkdir -p "$pkg/bin"
cp src/valkey-server src/valkey-cli "$pkg/bin/"
strip "$pkg/bin/valkey-server" "$pkg/bin/valkey-cli"
cp COPYING "$pkg/"
"$pkg/bin/valkey-server" --version
"$pkg/bin/valkey-cli" --version

archive="$out/valkey-$version-$platform.tar.gz"
cd "$work/package"
tar czf "$archive" "valkey-$version"
echo "$(sha "$archive")  $(wc -c < "$archive" | tr -d ' ')  $(basename "$archive")"
