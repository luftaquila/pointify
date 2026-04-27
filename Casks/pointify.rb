cask "pointify" do
  version "2.8.0"

  on_arm do
    sha256 "4ebb5c26824ae8baf0cb74cc1ccb1f03782bb1f33760f86231c0c5d640c38922"
    url "https://github.com/luftaquila/pointify/releases/download/v#{version}/pointify-macos-aarch64.dmg"
  end

  on_intel do
    sha256 "6ccca8e70bfe5dd04167730367cc4611691b13681b73d73407a6ee5a4b6701b8"
    url "https://github.com/luftaquila/pointify/releases/download/v#{version}/pointify-macos-x86_64.dmg"
  end

  name "Pointify"
  desc "Retro analog gauge meter system for hardware monitoring"
  homepage "https://github.com/luftaquila/pointify"

  app "Pointify.app"

  postflight do
    system_command "/usr/bin/xattr",
                   args: ["-dr", "com.apple.quarantine", "#{appdir}/Pointify.app"],
                   sudo: false
  end
end
