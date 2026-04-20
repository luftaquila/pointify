cask "pointify" do
  version "2.6.0"

  on_arm do
    sha256 "a580369c07e6eb983a9ae85f011315a7b67ac1161b66cc2613f5d4956c5dee6d"
    url "https://github.com/luftaquila/pointify/releases/download/v#{version}/pointify-macos-aarch64.dmg"
  end

  on_intel do
    sha256 "da9dd6644729e9a64307258b6ce90938b6038c52eb7366ea472591725374f0b5"
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
