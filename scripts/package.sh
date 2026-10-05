#!/bin/sh
# Pack a built `nsm` for distribution.
#
#   scripts/package.sh NAME VERSION BINARY
#
# Writes dist/nsm-VERSION-NAME.tar.gz, which unpacks to one directory of the
# same name holding the binary, README.md, CHANGELOG.md, LICENSE and docs/,
# and dist/nsm-VERSION-NAME.tar.gz.sha256 next to it. NAME is the platform:
# the target triple, with a suffix when one triple has more than one build
# (x86_64-unknown-linux-gnu-glibc2.17). Run from the repository root; the
# binaries workflow (.github/workflows/binaries.yml) runs it for every
# platform, and a release attaches what it wrote.
set -eu

if [ $# -ne 3 ]; then
    echo "usage: $0 NAME VERSION BINARY" >&2
    exit 2
fi
name=$1
version=$2
binary=$3

pkg="nsm-$version-$name"
rm -rf "dist/$pkg"
mkdir -p "dist/$pkg"
cp "$binary" "dist/$pkg/nsm"
cp README.md CHANGELOG.md LICENSE "dist/$pkg/"
cp -R docs "dist/$pkg/docs"
# COPYFILE_DISABLE keeps macOS tar from adding ._ resource-fork entries.
COPYFILE_DISABLE=1 tar -C dist -czf "dist/$pkg.tar.gz" "$pkg"
rm -rf "dist/$pkg"

cd dist
if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$pkg.tar.gz" > "$pkg.tar.gz.sha256"
else
    shasum -a 256 "$pkg.tar.gz" > "$pkg.tar.gz.sha256"
fi
cat "$pkg.tar.gz.sha256"
