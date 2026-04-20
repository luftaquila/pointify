cask "pointify" do
  version "2.5.1"

  on_arm do
    sha256 "803d487b8d3dc7b92b3a8e6a0838690593227aac33dfa935f5a540b5ca336ae4"
    url "https://github.com/luftaquila/pointify/releases/download/v#{version}/pointify-macos-aarch64.dmg"
  end

  on_intel do
    sha256 "2bc3d32cf6b0f48dbf8938d3a915f5ab9f488fe9dc73399b72de086362affb43"
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
