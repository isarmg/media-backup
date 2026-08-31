# 第 2 章：开发环境与第一次运行

## 基础工具

服务端需要 Rust/Cargo、C 编译工具链和 SQLite 构建依赖。Android 需要 JDK、Android SDK API 36、build
tools 36、NDK `28.2.13676358` 与 `cargo-ndk`。iOS 需要 macOS、Xcode、XcodeGen 和 Apple Rust target。

先确认工作区：

```bash
rustc --version
cargo metadata --locked --no-deps --format-version 1
cargo check --workspace --locked
```

不要用 `cargo update` 解决本机编译问题；它会改变锁图。工具链或平台缺失应先修复环境。

## 服务端开发配置

`config/media-backup.env.example` 是字段说明，不应直接变成生产 Secret 文件。开发环境准备独立临时数据库和数据目录，
设置强随机管理员密码。只有 `DEVELOPMENT=true` 且 `BIND` 是 loopback 时才可关闭 HTTPS 强制。

```bash
cargo run -p media-backup-server -- serve
curl --fail http://127.0.0.1:8080/health
```

若 source-bound binary 报告拒绝 `serve`，说明你运行的是正式身份，不应绕过；改用普通开发构建或完整
发行树的 `serve-release`。

## 初始化后的第一条业务链

1. 浏览器打开 `/admin`。
2. 使用全新数据库初始化的管理员登录。
3. 创建一个普通用户，不让移动端使用管理员身份。
4. 移动端配置 HTTPS 服务地址和普通用户凭据。
5. 授予系统照片权限、选择相册、启动一次扫描。
6. 在服务端确认资产、对象和审计，再在另一设备恢复测试媒体。

## Android 构建

```powershell
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
cargo install cargo-ndk
.\scripts\build-android-rust.ps1
gradle -p android testDebugUnitTest assembleDebug
```

构建脚本必须生成 `libmedia_backup_mobile.so`，Kotlin package 为 `com.example.mediabackup`，application ID
为当前 v0.2 身份。旧 native library 不得留在 `jniLibs` 冒充当前实现。

## iOS 构建

```bash
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
./scripts/build-ios-rust.sh
cd ios
xcodegen generate
xcodebuild -project MediaBackup.xcodeproj -scheme MediaBackup \
  -sdk iphonesimulator CODE_SIGNING_ALLOWED=NO build
```

`clients/ios/project.yml` 是 Xcode project 的事实源；生成的 `.xcodeproj` 和 Info.plist 不提交。权限文案、bundle
ID、entitlements 和测试 target 都由 project.yml 统一生成。

## 常见首次运行错误

| 现象 | 原因 | 处理 |
|---|---|---|
| 明文地址被移动端拒绝 | 当前客户端只接受 HTTPS | 使用本地受信证书或反向代理 |
| 服务端拒绝启动 | 旧/空数据库文件或 Schema 漂移 | 新建当前库；历史数据交给升级工具 |
| Android 找不到 JNI | ABI 目录或 library 名不符 | 重跑 Rust Android 构建并核对当前文件名 |
| iOS 链接不到符号 | header/FFI epoch 与 Swift 名不一致 | 运行移动契约门禁，禁止旧符号 fallback |
| 登录 429 | 来源/账户预算或 Argon2 并发耗尽 | 等待 Retry-After，检查压力而非关闭限流 |
