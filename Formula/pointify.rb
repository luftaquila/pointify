class Pointify < Formula
  desc "Retro analog gauge meter system for hardware monitoring"
  homepage "https://github.com/luftaquila/pointify"
  version "2.9.2"
  license "MIT"

  on_linux do
    on_arm do
      url "https://github.com/luftaquila/pointify/releases/download/v#{version}/pointify-linux-aarch64.AppImage"
      sha256 "99b9bb8ff376141d66bd9eda2ec56bbc9db9e784e132fb9d9d0b51dd9f190704"
    end

    on_intel do
      url "https://github.com/luftaquila/pointify/releases/download/v#{version}/pointify-linux-x86_64.AppImage"
      sha256 "a2e0f4ee99f756f3d11a75ce624a3aff660789b1bbc987377fef49772bd71741"
    end
  end

  def install
    bin.install Dir["pointify-linux-*.AppImage"].first => "pointify"
  end
end
