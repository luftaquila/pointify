cask "pointify" do
  version "2.9.0"

  on_arm do
    sha256 "a0a679f41a3ed6c7b3c3ffa13bd50cad1fa92e4d57e1c1699380a5022349f030"
    url "https://github.com/luftaquila/pointify/releases/download/v#{version}/pointify-macos-aarch64.dmg"
  end

  on_intel do
    sha256 "e1e6df9ae34a9c0ebecbd02d53675759651682ecf2c2cd9aeba9b6df57ac914c"
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
