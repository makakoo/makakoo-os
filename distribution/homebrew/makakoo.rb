# Makakoo OS Homebrew formula
#
# SHA256 placeholders below are filled in at release time by cargo-dist
# (or by hand if CI is still deferred). Do NOT commit real SHAs until
# the binaries for that version are actually published.
class Makakoo < Formula
  desc "Makakoo OS — autonomous cognitive extension for any AI CLI"
  homepage "https://makakoo.com"
  version "0.1.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/makakoo/makakoo-os/releases/download/v#{version}/makakoo-aarch64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER_SHA256_DARWIN_ARM64"
    end
    on_intel do
      url "https://github.com/makakoo/makakoo-os/releases/download/v#{version}/makakoo-x86_64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER_SHA256_DARWIN_INTEL"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/makakoo/makakoo-os/releases/download/v#{version}/makakoo-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "PLACEHOLDER_SHA256_LINUX_X86_64"
    end
  end

  def install
    bin.install "makakoo"
    bin.install "makakoo-mcp"
  end

  test do
    assert_match "makakoo 0.1.0", shell_output("#{bin}/makakoo version")
    assert_match "\"ok\":true", shell_output("#{bin}/makakoo-mcp --health")
  end
end
