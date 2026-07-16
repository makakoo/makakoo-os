# Makakoo OS Homebrew formula — source of truth.
#
# This file mirrors the live formula at
#   github.com/traylinx/homebrew-tap/Formula/makakoo.rb
# Bump `version` + the four `sha256` lines at each release. SHAs come
# from the .sha256 sidecar files attached to the GitHub release:
#
#   gh release download v<NEW> -R makakoo/makakoo-os \
#     --pattern '*.sha256' --output -
#
# Then mirror this file into the homebrew-tap repo (see
# `docs/RELEASING.md`). Until the live tap is bumped, brew users stay
# on the previous version.
class Makakoo < Formula
  desc "Autonomous cognitive extension for any AI CLI"
  homepage "https://makakoo.com"
  version "0.1.39"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/makakoo/makakoo-os/releases/download/v#{version}/makakoo-aarch64-apple-darwin.tar.gz"
      sha256 "1feb2d2f26baaa35a6acb374473a09bf08afaa4419285c9a04f37a93b479e20d"
    end
    on_intel do
      url "https://github.com/makakoo/makakoo-os/releases/download/v#{version}/makakoo-x86_64-apple-darwin.tar.gz"
      sha256 "1fa8b2d41d903c105eec79ee2a87d720fc85dcd507bdb644d053cff5a8348702"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/makakoo/makakoo-os/releases/download/v#{version}/makakoo-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "a3b76f29bb56dd7ab6fe79ccd05ceff0a956d263ac60f3ab7a3d93f70ec68eb1"
    end
    on_arm do
      url "https://github.com/makakoo/makakoo-os/releases/download/v#{version}/makakoo-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "0f2ff691a5700b5bbae5c4689e989abffd4fb48cdd2c263f441e47a8bf260351"
    end
  end

  def install
    bin.install "makakoo"
    bin.install "makakoo-mcp"
    # Bundled runtime data — the binary's resolve_distros_dir() and
    # plugins_core_root() fall back to <exe>/../share/makakoo/ which
    # under Homebrew resolves to #{prefix}/share/makakoo/ (= pkgshare).
    pkgshare.install "distros"
    pkgshare.install "plugins-core"
  end

  test do
    assert_match "makakoo #{version}", shell_output("#{bin}/makakoo --version")
  end
end
