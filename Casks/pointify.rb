cask "pointify" do
  version "2.9.0"

  on_arm do
    sha256 "d9565fa6eec19f21ff03220d994e9f15661295dec89bd6845b250187c2d89578"
    url "https://github.com/luftaquila/pointify/releases/download/v#{version}/pointify-macos-aarch64.dmg"
  end

  on_intel do
    sha256 "3b7f9348f7022081a77ad9688246e8d4c23989c9fb139152b748c60aa0d5f97d"
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
