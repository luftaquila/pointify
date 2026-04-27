class Pointify < Formula
  desc "Retro analog gauge meter system for hardware monitoring"
  homepage "https://github.com/luftaquila/pointify"
  version "2.8.0"
  license "MIT"

  on_linux do
    on_arm do
      url "https://github.com/luftaquila/pointify/releases/download/v#{version}/pointify-linux-aarch64.AppImage"
      sha256 "92b250a9cf343bbabc5aa7e4a9f379a7f10aca4d34d977cbc8679b99892b1773"
    end

    on_intel do
      url "https://github.com/luftaquila/pointify/releases/download/v#{version}/pointify-linux-x86_64.AppImage"
      sha256 "463e6a7506770265a0994ee50963a35ea605e9bfb1fb215afdc1bf314fc1c500"
    end
  end

  def install
    bin.install Dir["pointify-linux-*.AppImage"].first => "pointify"
  end
end
