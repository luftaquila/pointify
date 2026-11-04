cask "pointify" do
  version "2.9.2"

  on_arm do
    sha256 "ba45d72cf1725b732b92f8b4635defc78c3e8a56ddb3845ad6eae0fcb9d026ca"
    url "https://github.com/luftaquila/pointify/releases/download/v#{version}/pointify-macos-aarch64.dmg"
  end

  on_intel do
    sha256 "eab2a08ce5c32a6089f3339d257909bac46c02c7948b4e91abed492c6101065a"
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
