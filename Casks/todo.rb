cask "todo" do
  version "1"
  sha256 "8af3ef9c777d40b8484e29fcfc65098e5e43895835d2bad696d9274d47e11d9f"

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
