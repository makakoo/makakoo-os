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
  desc "Makakoo OS — autonomous cognitive extension for any AI CLI"
  homepage "https://makakoo.com"
  version "0.1.7"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/makakoo/makakoo-os/releases/download/v#{version}/makakoo-aarch64-apple-darwin.tar.gz"
      sha256 "1b99ce259abbec3ce9eee26ae45ba3aad570d478190408a16c8c77166b14b65a"
    end
    on_intel do
      url "https://github.com/makakoo/makakoo-os/releases/download/v#{version}/makakoo-x86_64-apple-darwin.tar.gz"
      sha256 "033a697c2f36f38d9c4dc42e6a9ee24a1406153e0a7630ac6ec3751f4221cfe5"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/makakoo/makakoo-os/releases/download/v#{version}/makakoo-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "79224c52203382c8f19f385cad8f04184319af3f0d930a0f7af9f80dcabbb0c0"
    end
    on_arm do
      url "https://github.com/makakoo/makakoo-os/releases/download/v#{version}/makakoo-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "fa62b44a18bdc37054d7e387e4655cef5e22370d7651d26d8db2d3988cdb02d2"
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
    assert_match "makakoo 0.1.7", shell_output("#{bin}/makakoo --version")
  end
end
