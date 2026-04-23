cask "pointify" do
  version "2.7.0"

  on_arm do
    sha256 "ac434fd70512e7cca77108881268b6c64abf7e4ae828b56c9c8c51889d861fee"
    url "https://github.com/luftaquila/pointify/releases/download/v#{version}/pointify-macos-aarch64.dmg"
  end

  on_intel do
    sha256 "17bd89a4d790bce7243a453f0ad0930fd6477af1bb2da24812729d7c07a67104"
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
