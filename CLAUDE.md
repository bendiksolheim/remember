# Working agreement

## Environment

You run in a Linux container with Rust only. There is no Swift toolchain and no
Apple SDK. Do not try to install either, accept the situation.

**You may:** write and test Rust; run `cargo xtask test`, `cov`, `ci`,
`bindings`; write Swift source files.

**You may not:** build or run the app. `cargo xtask mac | run | sim | device`
require macOS and will refuse to run here.

## Non-negotiables

- `todo-core` stays at 100% line coverage. Tests ship with the code.
- No business logic in Swift. If a view needs to compute something, it
  almost certainly belongs in `todo-core`. The only exception is if
  something is only needed for one target (iOS or macOS).
- No `.xcodeproj`. No `binaryTarget`. No XCFramework.
- Never edit generated files: `swift/Sources/TodoKit/todo_ffi.swift`,
  `swift/Sources/TodoFFI/*`.
