cask "remember" do
  version "6"
  sha256 "a6790f53566fe1152660044f0cf6ec2185e0a8243e48c3c0a213ef00be9ae2e3"

  url "https://github.com/bendiksolheim/remember/releases/download/v#{version}/Remember-#{version}-macos-arm64.zip"
  name "Remember"
  desc "Local-first task app with device sync"
  homepage "https://github.com/bendiksolheim/remember"

  depends_on macos: ">= :sonoma"
  depends_on arch: :arm64

  app "Remember.app"

  postflight do
    system_command "/usr/bin/xattr",
                    args: ["-cr", "#{appdir}/Remember.app"]
  end

  # The sync session lives in the login keychain, not under ~/Library.
  # `security` exits non-zero when there is no item (never signed in).
  # These identifiers still use the app's original name, "todo".
  zap script: {
        executable:   "/usr/bin/security",
        args:         ["delete-generic-password", "-s", "no.bendik.todo.sync", "-a", "session"],
        must_succeed: false,
      },
      trash:  [
        "~/Library/Application Support/no.bendik.todo",
      ]
end
