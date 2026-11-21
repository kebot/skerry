cask "skerry" do
  arch arm: "arm64"

  version "0.0.1"
  # Replace :no_check with the SHA-256 of Skerry-<version>-arm64.zip from SHA256SUMS on release.
  sha256 :no_check

  url "https://github.com/kebot/skerry/releases/download/v#{version}/Skerry-#{version}-#{arch}.zip"
  name "Skerry"
  desc "Native macOS client for SealSkin and Selkies"
  homepage "https://github.com/kebot/skerry"

  depends_on macos: ">= :ventura"
  depends_on arch: :arm64

  app "Skerry.app"

  caveats <<~EOS
    Skerry is distributed as an ad-hoc signed build without Apple notarization.
    Install with:
      brew install --cask --no-quarantine skerry

    If macOS Gatekeeper blocks launch, clear the quarantine attribute:
      xattr -cr /Applications/Skerry.app
  EOS

  zap trash: [
    "~/Library/Application Support/Skerry",
    "~/Library/Preferences/dev.kebot.skerry.plist",
  ]
end
