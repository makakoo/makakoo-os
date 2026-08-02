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
  version "0.2.2"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/makakoo/makakoo-os/releases/download/v#{version}/makakoo-aarch64-apple-darwin.tar.gz"
      sha256 "95be4fbd13febe20d2dc870c4eeda209c6eab379d03bff35d9e6cd9e71b0e99c"
    end
    on_intel do
      url "https://github.com/makakoo/makakoo-os/releases/download/v#{version}/makakoo-x86_64-apple-darwin.tar.gz"
      sha256 "e73ed9053b29518a42228c974782a7add089776751b6ef456ebad4c96cd264d4"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/makakoo/makakoo-os/releases/download/v#{version}/makakoo-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "ee023f6feb2f8feeca3c7c6946a535250f03733fabf69fa9d3250ce1ab9ae703"
    end
    on_arm do
      url "https://github.com/makakoo/makakoo-os/releases/download/v#{version}/makakoo-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "a22effc321c232a93dc1b6cc5a47c5a6671dd93991d6c0ce7bd5cae11a5b7617"
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
