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
  version "0.1.40"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/makakoo/makakoo-os/releases/download/v#{version}/makakoo-aarch64-apple-darwin.tar.gz"
      sha256 "b060965fbd27e77029e746d8d1a5937bb7b0d1b594d600957d3e19701579df9d"
    end
    on_intel do
      url "https://github.com/makakoo/makakoo-os/releases/download/v#{version}/makakoo-x86_64-apple-darwin.tar.gz"
      sha256 "ed37e25833d8a17b3e8bec975da987d0b8da061510a750ad492c637bcee71596"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/makakoo/makakoo-os/releases/download/v#{version}/makakoo-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "3234035c86924103c4d67b395d9b2c8700045c08173f3b8ac53760bb829bab1d"
    end
    on_arm do
      url "https://github.com/makakoo/makakoo-os/releases/download/v#{version}/makakoo-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "781152ff51b70bfa1db7636cb22d961c458bbccdca7689bb039d1db30ae85eb5"
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
