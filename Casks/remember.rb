cask "remember" do
  version "4"
  sha256 "6bdaecf8dd0f30d93030a1ca54174f15d474cd4f980a0083dd8c949a8c73e512"

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
