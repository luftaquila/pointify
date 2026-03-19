class Pointify < Formula
  desc "Retro analog gauge meter system for hardware monitoring"
  homepage "https://github.com/luftaquila/pointify"
  version "2.4.1"
  license "MIT"

  on_linux do
    on_arm do
      url "https://github.com/luftaquila/pointify/releases/download/v#{version}/pointify-linux-aarch64.AppImage"
      sha256 "23694409e3c2aebd01ac7fc1061a8efdae14cabe91a686457f1ef8f61004b6ee"
    end

    on_intel do
      url "https://github.com/luftaquila/pointify/releases/download/v#{version}/pointify-linux-x86_64.AppImage"
      sha256 "c87d24b3c8fffa02dda39419d6d6335139a0bba7332cc9c06e103f0845d8423f"
    end
  end

  def install
    bin.install Dir["pointify-linux-*.AppImage"].first => "pointify"
  end
end
