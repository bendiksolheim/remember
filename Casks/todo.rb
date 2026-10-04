cask "todo" do
  version "4"
  sha256 "6bdaecf8dd0f30d93030a1ca54174f15d474cd4f980a0083dd8c949a8c73e512"

  url "https://github.com/bendiksolheim/remember/releases/download/v#{version}/Todo-#{version}-macos-arm64.zip"
  name "Todo"
  desc "Local-first todo app with device sync"
  homepage "https://github.com/bendiksolheim/remember"

  depends_on macos: ">= :sonoma"
  depends_on arch: :arm64

  app "Todo.app"

  postflight do
    system_command "/usr/bin/xattr",
                    args: ["-cr", "#{appdir}/Todo.app"]
  end

  zap trash: [
    "~/Library/Application Support/no.bendik.todo",
  ]
end
