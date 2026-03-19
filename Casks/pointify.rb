cask "pointify" do
  version "2.4.1"

  on_arm do
    sha256 "b45d7e07d72bc65015ead843bbb781cae764727b37977d3cd57437e6a3b5468d"
    url "https://github.com/luftaquila/pointify/releases/download/v#{version}/pointify-macos-aarch64.dmg"
  end

  on_intel do
    sha256 "ec489669014b3069a2403d339ebeb9e553757f39811bc11cea36ae44687c7747"
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
