# Homebrew formula for Pulsate. Drop this into a tap repo named `squaretick/homebrew-tap`
# (file: Formula/pulsate.rb), then: `brew install squaretick/tap/pulsate`.
#
# Update `version` and the four `sha256` values on each release. The checksums
# are printed by the `release` workflow (taiki-e `checksum: sha256`) and also
# attached to the GitHub release as `*.sha256` files. `brew bump-formula-pr`
# can automate the bump.
class Pulsate < Formula
  desc "Reverse-proxy gateway: TLS, caching, WAF, observability, admin API, WASM plugins"
  homepage "https://github.com/squaretick/pulsate"
  version "0.1.0"
  license "Apache-2.0"

  on_macos do
    on_arm do
      url "https://github.com/squaretick/pulsate/releases/download/v#{version}/pulsate-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_AARCH64_DARWIN_SHA256"
    end
    on_intel do
      url "https://github.com/squaretick/pulsate/releases/download/v#{version}/pulsate-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_X86_64_DARWIN_SHA256"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/squaretick/pulsate/releases/download/v#{version}/pulsate-v#{version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_AARCH64_LINUX_SHA256"
    end
    on_intel do
      url "https://github.com/squaretick/pulsate/releases/download/v#{version}/pulsate-v#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_X86_64_LINUX_SHA256"
    end
  end

  def install
    bin.install "pulsate"
  end

  test do
    assert_match "pulsate", shell_output("#{bin}/pulsate --version")
  end
end
