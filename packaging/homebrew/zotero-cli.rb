# Homebrew formula for zotero-cli (zotero-rust-cli project).
#
# Intended to live in a tap repo, e.g. ntluong95/homebrew-zotero-rust-cli,
# as Formula/zotero-cli.rb. `brew install` there strips the macOS
# quarantine attribute automatically, so this is the primary supported
# macOS install path (no paid Apple Developer signing required).
#
# The two `sha256` placeholders below must be filled in per release from
# dist/SHA256SUMS after `.github/workflows/release.yml` publishes the
# aarch64-apple-darwin and x86_64-apple-darwin archives.
class ZoteroCli < Formula
  desc "Native CLI for AI agents to work with Zotero libraries (Rust port of cli-anything-zotero)"
  homepage "https://github.com/ntluong95/zotero-rust-cli"
  version "0.1.0"
  license "Apache-2.0"

  on_macos do
    on_arm do
      url "https://github.com/ntluong95/zotero-rust-cli/releases/download/v#{version}/zotero-cli-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_AARCH64_APPLE_DARWIN_SHA256"
    end
    on_intel do
      url "https://github.com/ntluong95/zotero-rust-cli/releases/download/v#{version}/zotero-cli-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_X86_64_APPLE_DARWIN_SHA256"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/ntluong95/zotero-rust-cli/releases/download/v#{version}/zotero-cli-v#{version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_AARCH64_UNKNOWN_LINUX_GNU_SHA256"
    end
    on_intel do
      url "https://github.com/ntluong95/zotero-rust-cli/releases/download/v#{version}/zotero-cli-v#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_X86_64_UNKNOWN_LINUX_GNU_SHA256"
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
