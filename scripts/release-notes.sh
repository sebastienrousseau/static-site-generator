#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0 OR MIT
#
# Write the release page body for one published tag.
#
#   scripts/release-notes.sh <tag> [<previous-tag>]
#
# The page has the sections every release of this project uses, in this
# order: the hand-written highlights from docs/releases/<tag>.md, What's
# Changed as GitHub generates it, the SHA-256 of every asset attached to
# the release, and the Full Changelog link. The previous tag defaults to
# the highest v* tag on origin below <tag>.
#
# release.yml runs this after uploading the assets and sets the result,
# with the title "ssg <version>", on the release. Before this script the
# workflow wrote a fixed Install/Verify body and the title "v<version>",
# so v0.0.73 and v0.0.74 shipped without highlights or checksums and had
# to be corrected by hand.
#
# Needs `gh` (authenticated, or GH_TOKEN) and GITHUB_REPOSITORY or a
# GH_REPO-style owner/name in REPO. Fails rather than publish a page
# with a missing section or an asset without a digest, and turns any
# em dash a PR title brings in into a hyphen.
set -euo pipefail

if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
  echo "usage: $0 <tag> [<previous-tag>]" >&2
  exit 2
fi
tag=$1
repo=${REPO:-${GITHUB_REPOSITORY:?set GITHUB_REPOSITORY or REPO}}
root=$(cd "$(dirname "$0")/.." && pwd)
highlights="$root/docs/releases/${tag}.md"

die() { echo "release-notes: $*" >&2; exit 1; }

[ -f "$highlights" ] || die "missing $highlights"
grep -qxF '## Highlights ⭐️' "$highlights" || die "$highlights has no '## Highlights ⭐️' heading"

prev=${2:-$(git -C "$root" ls-remote --tags --refs origin 'v*' \
  | sed 's|.*refs/tags/||' | sort -V | awk -v t="$tag" '$0 == t { print p; exit } { p = $0 }')}
[ -n "$prev" ] || die "no tag found before $tag"

changes=$(gh api "repos/${repo}/releases/generate-notes" \
  -f tag_name="$tag" -f previous_tag_name="$prev" --jq .body \
  | sed '/^\*\*Full Changelog\*\*/,$d')
grep -q "^## What's Changed" <<<"$changes" || die "generated notes have no What's Changed section"

sums=$(gh api "repos/${repo}/releases/tags/${tag}" \
  --jq '.assets[] | "\(.digest // "")  \(.name)"' | LC_ALL=C sort -k2)
[ -n "$sums" ] || die "release $tag has no assets"
if grep -qvE '^sha256:[0-9a-f]{64}  ' <<<"$sums"; then
  die "an asset has no SHA-256 digest: $(grep -vE '^sha256:' <<<"$sums" | head -1)"
fi

body=$(
  cat "$highlights"
  printf '\n%s\n' "$(printf '%s\n' "$changes" | sed -e :a -e '/^\n*$/{$d;N;ba' -e '}')"
  printf '\n## Checksums\n\nSHA-256 of every asset attached to this release:\n\n```text\n'
  printf '%s\n' "${sums//sha256:/}"
  printf '```\n\n**Full Changelog**: https://github.com/%s/compare/%s...%s\n' "$repo" "$prev" "$tag"
)
# PR titles in What's Changed may carry em dashes; release pages do not.
body=${body//—/-}
printf '%s\n' "$body"
