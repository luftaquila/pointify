class Pointify < Formula
  desc "Retro analog gauge meter system for hardware monitoring"
  homepage "https://github.com/luftaquila/pointify"
  version "2.9.0"
  license "MIT"

  on_linux do
    on_arm do
      url "https://github.com/luftaquila/pointify/releases/download/v#{version}/pointify-linux-aarch64.AppImage"
      sha256 "3b1e7152b01f3986f8e817f49f7dbbdc994a63bc37188c1d58637c51236a499b"
    end

    on_intel do
      url "https://github.com/luftaquila/pointify/releases/download/v#{version}/pointify-linux-x86_64.AppImage"
      sha256 "d130de586f524e08315f9f3d17a1468f85ec510d650cc24266f5df9205978c3b"
    end
  end

  def install
    bin.install Dir["pointify-linux-*.AppImage"].first => "pointify"
  end
end
