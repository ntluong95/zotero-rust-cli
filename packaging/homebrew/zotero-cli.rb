# frozen_string_literal: true

# Homebrew formula for zotero-cli (zotero-rust-cli project).
#
# Intended to live in a tap repo, e.g. ntluong95/homebrew-zotero-rust-cli,
# as Formula/zotero-cli.rb. `brew install` there strips the macOS
# quarantine attribute automatically, so this is the primary supported
# macOS install path (no paid Apple Developer signing required).
#
# The `sha256` values below are from the actual v0.1.0 release's
# SHA256SUMS (verified against the published GitHub Release). Bump
# `version` and regenerate these from dist/SHA256SUMS for every release.
class ZoteroCli < Formula
  desc "Native CLI for AI agents to work with Zotero libraries"
  homepage "https://github.com/ntluong95/zotero-rust-cli"
  version "0.1.0"
  license "Apache-2.0"

  on_macos do
    on_arm do
      url "https://github.com/ntluong95/zotero-rust-cli/releases/download/v#{version}/zotero-cli-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "34df43d7778f69ed912de64721404656022258a877a450fbd8376cab289aa737"
    end
    on_intel do
      url "https://github.com/ntluong95/zotero-rust-cli/releases/download/v#{version}/zotero-cli-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "26e5a59e40ebcc470dac6c4fd1ab33b144418a3394420b96b761b78f1a3851be"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/ntluong95/zotero-rust-cli/releases/download/v#{version}/zotero-cli-v#{version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "00e98f9adf9b3aa8937a7902836e83635c8c64f43eefdc947ed94c6eaabcd8f8"
    end
    on_intel do
      url "https://github.com/ntluong95/zotero-rust-cli/releases/download/v#{version}/zotero-cli-v#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "40e3b34a4f62f1e4b7de7f22f227801f00dedb37a78a800acace685c299817f7"
    end
  end

  def install
    bin.install "zotero-cli"
    bin.install "cli-anything-zotero"
    doc.install "LICENSE", "NOTICE-CHANGES.md", "THIRD-PARTY-LICENSES.md"
  end

  test do
    assert_match "zotero-cli", shell_output("#{bin}/zotero-cli")
  end
end
