cask "pointify" do
  version "2.9.1"

  on_arm do
    sha256 "bfcd0f7b36320a27d2f32daaccf2532fe1272b5973ae124653c0448ebecf1bea"
    url "https://github.com/luftaquila/pointify/releases/download/v#{version}/pointify-macos-aarch64.dmg"
  end

  on_intel do
    sha256 "f0ff575c38b42ca7a3ee875e69daa368a365ca2640aa6e5479d548478600bd1e"
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
