cask "pointify" do
  version "2.4.0"

  on_arm do
    sha256 "d262419ff3316c32d909d6d37e78535d28002ec8bbb6a1be13f4958205638db7"
    url "https://github.com/luftaquila/pointify/releases/download/v#{version}/pointify-macos-aarch64.dmg"
  end

  on_intel do
    sha256 "49392096cc3dfe568e284a16a59c7ff56ccd4fd152fa25283ada07a944d97ee1"
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
