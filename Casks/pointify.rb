cask "pointify" do
  version "2.9.2"

  on_arm do
    sha256 "f6c2d95f549eb13911ca5390b3a023c1f87c410f5787a451ee4ffbb3295e0f34"
    url "https://github.com/luftaquila/pointify/releases/download/v#{version}/pointify-macos-aarch64.dmg"
  end

  on_intel do
    sha256 "59a805cfdfd1f719d0c15e8f75e293a2f84fedb011c469684cfc25559d8a74c0"
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
