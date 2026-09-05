# 第 2 章：开发环境与第一次运行

## 基础工具

服务端固定 Rust `1.98.0`（minimal + rustfmt/clippy），管理 Web 固定 Node `26.7.0`、React `19.2.8`、
TypeScript strict 与 Vite `7.3.6`。Android 另需 JDK、Android SDK API 36、build tools 36、NDK
`28.2.13676358` 与 `cargo-ndk`；iOS 需要 macOS、Xcode、XcodeGen 和 Apple Rust target。

先确认工作区：

```bash
rustc --version
npm ci --prefix clients/web
npm run build --prefix clients/web
cargo metadata --locked --no-deps --format-version 1
cargo check --workspace --locked
```

`npm run build` 会先执行 `check:foundation`，并生成 Server 编译时嵌入的 dist；因此 Cargo 之前必须完成
Web 构建。不要用 `cargo update` 或宽版本范围解决本机问题，它们会改变锁图或绕过统一基线。

## 服务端开发配置

`config/media-backup.env.example` 是字段说明，不应直接变成生产 Secret 文件。开发环境准备独立临时数据库和数据目录，
设置规范化管理员 `BOOTSTRAP_ADMIN_USERNAME` 和强随机 `BOOTSTRAP_ADMIN_PASSWORD`。默认样例为 `admin`；服务不会读取
`ADMIN_EMAIL`。username 候选经 ASCII trim/lowercase 后必须是 3–64 bytes、首尾字母数字且仅含
`[a-z0-9._-]`。只有
`DEVELOPMENT=true` 且 `BIND` 是 loopback 时才可关闭 HTTPS 强制。

```bash
cargo run -p media-backup-server -- serve
curl --fail http://127.0.0.1:8080/healthz
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

第 2 步的管理 username 只供 `/api/v2/auth/*`；第 3–5 步的普通账户继续使用 `accounts.username` 与
移动端 `/v2` 合同。不要把管理员密码填入 Android/iOS，也不要因为两个 username 同名而把它们视为同一身份。

## Android 构建

```powershell
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
cargo install cargo-ndk
.\scripts\build-android-rust.ps1
gradle -p clients/android testDebugUnitTest assembleDebug
```

构建脚本必须生成 `libmedia_backup_mobile.so`；Kotlin namespace 与 application ID 都是
`org.sarmg.mediabackup`。`jniLibs` 只允许放置本次从当前 Rust workspace 生成的正式 ABI 文件。
这个本地命令只验证 Debug 构建。正式 `assembleRelease` 必须显式提供当前 PKCS#12 路径和密码；开发者
机器没有正式 Secret 时应当失败，不能自动落回 Debug key。证书 alias、SHA-256 和 APK application ID 由
release workflow 再次独立验证。

## iOS 构建

```bash
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
./scripts/build-ios-rust.sh
cd clients/ios
xcodegen generate
xcodebuild -project MediaBackup.xcodeproj -scheme MediaBackup \
  -sdk iphonesimulator CODE_SIGNING_ALLOWED=NO build
```

`clients/ios/project.yml` 是 Xcode project 的事实源；生成的 `.xcodeproj` 和 Info.plist 不提交。权限文案、bundle
ID `org.sarmg.mediabackup`、entitlements 和测试 target 都由 project.yml 统一生成。

## 常见首次运行错误

| 现象 | 原因 | 处理 |
|---|---|---|
| 明文地址被移动端拒绝 | 当前客户端只接受 HTTPS | 使用本地受信证书或反向代理 |
| 服务端拒绝启动 | 空数据库文件、非当前身份或 Schema 漂移 | 新建当前库；需要转换的数据交给升级工具 |
| Android 找不到 JNI | ABI 目录或 library 名不符 | 重跑 Rust Android 构建并核对当前文件名 |
| iOS 链接不到符号 | header/FFI epoch 与 Swift 名不一致 | 运行移动契约门禁，只保留当前符号 |
| 登录 429 | 来源/账户预算或 Argon2 并发耗尽 | 等待 Retry-After，检查压力而非关闭限流 |
