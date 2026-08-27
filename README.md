# 照片与视频备份

这是一个自托管的端到端加密照片备份项目。Rust 负责服务端、共享协议、客户端状态机、去重和加密；Swift 与 Kotlin 只负责访问系统照片库和调度后台上传。

## 已实现能力

- iOS PhotoKit 与 Android MediaStore 扫描。
- SQLite 持久化 Agent 状态机，进程中断后可恢复。
- XChaCha20-Poly1305 分块加密，账户内 HMAC 去重。
- PostgreSQL 元数据与本地对象目录。
- 幂等上传、缺失分块查询、服务端完整性校验和原子提交。
- iOS 后台 URLSession 与 Android WorkManager。
- 设备注册、独立 Bearer Token、资源清单和加密原文下载。
- 默认备份语义：手机删除不会删除服务端副本。

## 在 WSL 中启动服务端

安装 Rust、PostgreSQL 和编译依赖（以 Debian/Ubuntu 为例）：

```bash
sudo apt-get update
sudo apt-get install -y rustup postgresql postgresql-client pkg-config libssl-dev build-essential
rustup default stable
sudo ./scripts/setup-wsl.sh
```

初始化脚本会创建 `.env`，默认管理员账号为 `admin`，并生成随机管理员密码：

```bash
grep '^ADMIN_' .env
./scripts/run-server-wsl.sh
# 或编译 release 版本并交给 systemd 后台运行：
cargo build --release -p photo-backup-server
sudo ./scripts/start-server-wsl.sh
```

服务会自动执行数据库迁移。健康检查地址为 `http://127.0.0.1:8080/health`。移动设备必须使用可访问的 HTTPS 地址；正式环境应在服务端前放置 Caddy、Nginx 或云负载均衡器终止 TLS。

Web 管理页面位于 `/admin`。管理员登录后创建普通用户账号和密码；Android/iOS 客户端首次登录会换取独立设备令牌，后续后台上传不再反复发送密码。

## Android

Android 工程位于 `android/`，目标 API 36，使用 AGP 9.3、Compose 和 WorkManager 2.11.2。先安装 Rust Android 目标与 `cargo-ndk`，然后在根目录执行：

```powershell
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
cargo install cargo-ndk
.\scripts\build-android-rust.ps1
```

随后用 Android Studio 打开 `android/`。应用内填写服务端 HTTPS 地址、用户账号和密码，授予照片和视频权限后点击“立即备份”。

## iOS

iOS 工程描述位于 `ios/project.yml`，需要 macOS、Xcode、XcodeGen 和 iOS Rust targets：

```bash
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
./scripts/build-ios-rust.sh
cd ios && xcodegen generate
open PhotoBackup.xcodeproj
```

在 Xcode 中选择开发团队并使用真机测试。系统可能只授予部分照片权限，应用只会备份当前可访问的项目。用户强制退出 App 后，iOS 会暂停后台传输，重新打开后幂等续传。

## GitHub Actions 构建与发布

推送到 GitHub 后，`.github/workflows/build.yml` 会执行 Rust 格式检查和测试、生成 Android Debug APK，并在 macOS runner 上完成 iOS 无签名编译。APK 和 iOS 构建目录会上传为 workflow artifacts；正式发布仍需在仓库 Secrets 中配置平台签名材料。

创建并推送与 `Cargo.toml` 中工作区版本一致的标签后，`.github/workflows/release.yml` 会自动创建 GitHub Release：

```bash
git tag -a v0.1.0 -m "Release v0.1.0"
git push origin v0.1.0
```

发布流程会重新运行 Rust 格式检查和测试，并生成 Linux x86_64 服务端、可直接安装的 Android Debug APK、iOS 未签名应用包及 `SHA256SUMS`。Android 与 iOS 包目前仅用于测试分发；应用商店或生产分发仍需配置各平台签名。每次发布前应同步修改 `Cargo.toml` 的 `workspace.package.version` 和 `ios/project.yml` 的 `MARKETING_VERSION`，Android 版本会自动从 Cargo 工作区版本派生。

## 密钥模型

首次运行时，每台设备生成并安全保存 32 字节主密钥和去重密钥。为了让多台设备访问同一个备份，生产部署需要增加二维码配对或恢复密钥导入流程，将同一账户主密钥安全传递给新设备。当前实现中每个新注册设备默认生成自己的密钥，因此设备可以备份，但不能直接解密另一台设备上传的数据。

服务端永远不会收到明文主密钥、明文文件哈希或明文资源内容。恢复接口返回加密 blob 和解密清单，应由持有账户密钥的 Agent 解密。

## API 摘要

- `POST /v1/auth/bootstrap`：使用普通用户账号密码登录并注册设备。
- `POST /v1/uploads`：创建或恢复上传会话。
- `PUT /v1/uploads/{id}/parts/{index}`：幂等上传分块。
- `GET /v1/uploads/{id}`：获取缺失分块。
- `POST /v1/uploads/{id}/complete`：校验并提交资源。
- `GET /v1/resources`：列出账户资源。
- `GET /v1/resources/{id}`：取得加密清单。
- `GET /v1/resources/{id}/content`：下载加密 blob。

## 生产部署注意事项

- 必须启用 HTTPS，并更换管理员密码和数据库密码。
- `DATA_DIR` 必须位于可靠、可快照的持久卷上；数据库和对象目录需要一起备份。
- 管理员密码和普通用户密码不应写入仓库；生产环境应使用足够长的随机密码。
- 大规模部署可在 `storage.rs` 的边界后接入 S3/MinIO，并用预签名分块上传降低 Server 带宽。
- 上架前需要补充隐私政策、数据删除入口、恢复密钥 UX 和平台商店权限声明。
