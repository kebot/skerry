cask "skerry" do
  arch arm: "arm64"

  version "0.0.1"
  sha256 "904defef49322259a1a6b815fdd7847f511cc8b1ed1c91557c628a1de49ad0dc"

  url "https://github.com/kebot/skerry/releases/download/v#{version}/Skerry-#{version}-#{arch}.zip"
  name "Skerry"
  desc "Native macOS client for SealSkin and Selkies"
  homepage "https://github.com/kebot/skerry"

  depends_on macos: :ventura
  depends_on arch: :arm64

  app "Skerry.app"

  caveats <<~EOS
    Skerry is distributed as an ad-hoc signed build without Apple notarization.
    Before first launch, clear the macOS Gatekeeper quarantine attribute:
      xattr -cr /Applications/Skerry.app
  EOS

  zap trash: [
    "~/Library/Application Support/Skerry",
    "~/Library/Preferences/dev.kebot.skerry.plist",
  ]
end
