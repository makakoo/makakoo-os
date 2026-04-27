# Makakoo OS Homebrew formula
#
# Pinned to v0.1.0 — first public release (2026-04-27). Update version
# + sha256 lines at each release. SHAs come from
#   gh release download <tag> --pattern '*.sha256' --output -
# in the makakoo/makakoo-os repo.
class Makakoo < Formula
  desc "Makakoo OS — autonomous cognitive extension for any AI CLI"
  homepage "https://makakoo.com"
  version "0.1.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/makakoo/makakoo-os/releases/download/v#{version}/makakoo-aarch64-apple-darwin.tar.gz"
      sha256 "ca6a8023cab9c0a7a02b69aa49834c8b4a43812ebb5268e9cb893e2b7d7bef5a"
    end
    on_intel do
      url "https://github.com/makakoo/makakoo-os/releases/download/v#{version}/makakoo-x86_64-apple-darwin.tar.gz"
      sha256 "d955143c60bd5bdb2b22f9778fa7d3893911e3f17c497afdd72cb8e3fb6083ac"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/makakoo/makakoo-os/releases/download/v#{version}/makakoo-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "6c1d9e10a6b9e3774f775200734faa1f1524ec8f0c5a877dfff8d77859a3a2cb"
    end
    on_arm do
      url "https://github.com/makakoo/makakoo-os/releases/download/v#{version}/makakoo-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "6c49b9fd0a41ba4339b9246d31b21c83fbc08bc78cd86b4d1f6e147fd260efc2"
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
    assert_match "makakoo 0.1.0", shell_output("#{bin}/makakoo --version")
  end
end
