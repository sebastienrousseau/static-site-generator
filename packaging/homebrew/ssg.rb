# typed: false
# frozen_string_literal: true

# SPDX-License-Identifier: Apache-2.0 OR MIT
#
# Homebrew formula for SSG, Static Site Generator.
#
# Rendered from packaging/homebrew/ssg.rb in static-site-generator by
# scripts/render-packaging.sh, which fills in the version and each
# archive's SHA-256 at release time. Each release attaches the result
# as ssg.rb, and github.com/sebastienrousseau/homebrew-tap copies it,
# so users install with:
#
#   brew install sebastienrousseau/tap/ssg

class Ssg < Formula
  desc "Fast, SEO-optimised, WCAG-compliant static site generator"
  homepage "https://github.com/sebastienrousseau/static-site-generator"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "https://github.com/sebastienrousseau/static-site-generator/releases/download/v@VERSION@/ssg-v@VERSION@-aarch64-apple-darwin.tar.gz"
      sha256 "@SHA256_aarch64-apple-darwin@"
    end
    on_intel do
      url "https://github.com/sebastienrousseau/static-site-generator/releases/download/v@VERSION@/ssg-v@VERSION@-x86_64-apple-darwin.tar.gz"
      sha256 "@SHA256_x86_64-apple-darwin@"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/sebastienrousseau/static-site-generator/releases/download/v@VERSION@/ssg-v@VERSION@-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "@SHA256_aarch64-unknown-linux-gnu@"
    end
    on_intel do
      url "https://github.com/sebastienrousseau/static-site-generator/releases/download/v@VERSION@/ssg-v@VERSION@-x86_64-unknown-linux-musl.tar.gz"
      sha256 "@SHA256_x86_64-unknown-linux-musl@"
    end
  end

  def install
    bin.install "ssg"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/ssg --version")
  end
end
