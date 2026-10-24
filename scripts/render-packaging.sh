#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0 OR MIT
#
# Render the Homebrew, Scoop and winget manifests for one release.
#
#   scripts/render-packaging.sh <version> <artifact-dir> <out-dir>
#
# The files under packaging/{homebrew,scoop,winget} are templates:
# @VERSION@ stands for the version and @SHA256_<target>@ for the
# SHA-256 of ssg-v<version>-<target>.<ext>, read from the `.sha256`
# file the release build writes next to each archive. release.yml runs
# this on the built artifacts and attaches the output to the release as
# ssg.rb, ssg.json and ssg.winget.yaml.
#
# A hand-edited manifest drifted for 32 releases: it pointed at the
# v0.0.37 archive with an empty hash. Rendering from the artifacts the
# release itself built leaves nothing to keep in step.
set -euo pipefail

[ "$#" -eq 3 ] || { echo "usage: $0 <version> <artifact-dir> <out-dir>" >&2; exit 2; }
version="$1" artifacts="$2" out="$3"
root="$(cd "$(dirname "$0")/.." && pwd)"

case "$version" in
  v*) echo "error: pass the version without a leading v: ${version#v}" >&2; exit 2 ;;
esac

# Print the SHA-256 recorded for one target's archive, or fail.
sha_for() {
  local target="$1" file sum
  for file in "$artifacts/ssg-v$version-$target.tar.gz.sha256" \
              "$artifacts/ssg-v$version-$target.zip.sha256"; do
    [ -f "$file" ] || continue
    sum="$(awk 'NR==1{print tolower($1)}' "$file")"
    if [[ "$sum" =~ ^[0-9a-f]{64}$ ]]; then
      printf '%s\n' "$sum"
      return 0
    fi
    echo "error: $file does not start with a SHA-256" >&2
    return 1
  done
  echo "error: no checksum for ssg-v$version-$target in $artifacts" >&2
  return 1
}

# Fill one template; fail if any placeholder is left.
render() {
  local template="$1" dest="$2" text placeholder target sum
  text="$(cat "$root/$template")"
  text="${text//@VERSION@/$version}"
  while [[ "$text" =~ @SHA256_([A-Za-z0-9_-]+)@ ]]; do
    placeholder="${BASH_REMATCH[0]}"
    target="${BASH_REMATCH[1]}"
    sum="$(sha_for "$target")"
    text="${text//$placeholder/$sum}"
  done
  if [[ "$text" =~ @[A-Z0-9_]+[A-Za-z0-9_-]*@ ]]; then
    echo "error: $template still has ${BASH_REMATCH[0]}" >&2
    return 1
  fi
  printf '%s\n' "$text" > "$dest"
  echo "rendered $dest"
}

mkdir -p "$out"
render packaging/homebrew/ssg.rb "$out/ssg.rb"
render packaging/scoop/ssg.json "$out/ssg.json"
render packaging/winget/ssg.yaml "$out/ssg.winget.yaml"
