cask "todo" do
  version "3"
  sha256 "ce30f7be0b08b28bd26f2bab34862ba92e0acb75ea4a828017f9a999a5d107c9"

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
