class Pointify < Formula
  desc "Retro analog gauge meter system for hardware monitoring"
  homepage "https://github.com/luftaquila/pointify"
  version "2.5.1"
  license "MIT"

  on_linux do
    on_arm do
      url "https://github.com/luftaquila/pointify/releases/download/v#{version}/pointify-linux-aarch64.AppImage"
      sha256 "d6dd4492493a7ea6dcde9f92f06bbf376e5172f2cb40938bd6f25d1f7af3b0a8"
    end

    on_intel do
      url "https://github.com/luftaquila/pointify/releases/download/v#{version}/pointify-linux-x86_64.AppImage"
      sha256 "85848efb8f303bb0d9b508ea61fb1f2116be0feb1b4a4c6244afd2ad8e074df9"
    end
  end

  def install
    bin.install Dir["pointify-linux-*.AppImage"].first => "pointify"
  end
end
