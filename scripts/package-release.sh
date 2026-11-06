#!/usr/bin/env bash
# Package a prebuilt executable; called identically by local builds and CI.
set -euo pipefail
target=${1:?usage: package-release.sh TARGET}
case "$target" in
  x86_64-unknown-linux-musl|aarch64-unknown-linux-musl|x86_64-apple-darwin|aarch64-apple-darwin) ;;
  *) echo "unsupported release target: $target" >&2; exit 2 ;;
esac
version=$(awk -F'"' '/^version = /{print $2; exit}' Cargo.toml)
executable="target/$target/release/hc-tui"
test -x "$executable"
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
mkdir -p dist
cp "$executable" "$stage/hc-tui"
cp LICENSE THIRD-PARTY-NOTICES.md CHANGELOG.md README.md install.sh "$stage/"
archive="hc-tui-$version-$target.tar.gz"
tar -czf "dist/$archive" -C "$stage" .
if command -v sha256sum >/dev/null 2>&1; then
  (cd dist && sha256sum "$archive" > "$archive.sha256")
else
  (cd dist && shasum -a 256 "$archive" > "$archive.sha256")
fi
printf '%s\n' "dist/$archive" "dist/$archive.sha256"
