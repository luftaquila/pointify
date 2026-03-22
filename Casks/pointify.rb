cask "pointify" do
  version "2.5.0"

  on_arm do
    sha256 "3fe4212677d5bdd2ff706e439e2bc393773c033267a57c3e1683b6267e00c02f"
    url "https://github.com/luftaquila/pointify/releases/download/v#{version}/pointify-macos-aarch64.dmg"
  end

  on_intel do
    sha256 "4f96447f448d3cdddbcae354206121fe50ccfc87da87c136975e28306e3be9e5"
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
