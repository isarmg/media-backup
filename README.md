# 照片与视频备份

这是一个以 Rust 为共享核心、面向 iOS/Android 的自托管照片备份系统。媒体通过 HTTPS 加密传输，服务端保存与手机原文件字节一致的未加密内容，便于新设备直接恢复、生成和展示设备侧缩略图，并进行图库组织。

## 存储与安全边界

- Android 与 iOS 客户端拒绝非 `https://` 的服务地址。
- `REQUIRE_HTTPS=true`（默认）时，登录、管理、媒体和指标接口只接受来自 `TRUSTED_PROXY_CIDRS` 中真实传输 peer 的 HTTPS 标记；`/health*` 可留给本机探针。
- Axum 建议只监听 `127.0.0.1`，由 Caddy/Nginx 终止 TLS；生产模式使用带 `Secure` 的 `__Host-photo_session`。
- 上传分块、合并对象和下载均校验 BLAKE3；`plain-v1` 对象落盘后是原始未加密字节。
- 密码使用 Argon2 哈希；设备令牌和 API Key 只保存 SHA-256 摘要；移动端凭据分别存入 Android 加密偏好和 iOS Keychain。
- 服务器管理员和取得数据卷访问权的人可以读取媒体。生产环境仍应使用磁盘/卷加密、最小权限和加密的异地备份。

管理员身份、随机 Session 和 Session 绑定的 CSRF 摘要持久化在本项目自己的 SQLite 中。Session 有空闲与绝对期限，退出会在数据库撤销当前登录；管理员密码、角色或启用状态变化会使该管理员的全部既有 Session 失效。浏览器管理接口只接受 Cookie 身份，设备与 API Key 接口只接受各自的 Bearer Token，两者不能互换。

管理员登录和设备 bootstrap 的请求体上限为 4 KiB。两条密码登录路径共享 Argon2 并发闸门与执行超时，并分别按真实来源 IP、规范化账户维护有容量和 TTL 上限的 Token Bucket；未知或停用账户也会执行同参数 dummy Argon2，限流响应带 `Retry-After`。

仅在显式 `DEVELOPMENT=true` 且 `BIND` 为 `127.0.0.1` 或 `::1` 时，才允许 `REQUIRE_HTTPS=false` 和不带 `Secure` 的开发 Cookie；生产配置拒绝这种组合。

## 已实现的图库能力

1. 新设备使用账号密码注册后，分页读取完整时间线，可单项或批量恢复原图/视频到系统照片库。
2. Android MediaStore 与 iOS PhotoKit 均可选择或排除设备相册，允许明确选择零个相册。
3. 选中相册向服务器单向同步；完整扫描会精确替换成员，分批扫描不会误删尚未遍历的成员。
4. 缩略图在设备侧生成并作为独立资源上传，Android/iOS 时间线使用鉴权下载并展示。
5. 时间线使用不透明游标分页；`/v1/sync` 提供单调序列的增量变更流，两端持久化同步游标。
6. 收藏、归档和标签已进入 SQLite 模型、筛选 API 和移动端操作；标签支持创建、批量设置、单项添加/移除。
7. 软删除进入回收站，可撤销；只有已进入回收站的资产才能永久删除。
8. 服务端按原始内容 BLAKE3/大小列出重复组，移动端显示重复组数量，可将多余副本移入回收站。
9. API Key 支持创建、列出和撤销；关键写操作进入审计日志；带独立令牌的 `/metrics` 输出 Prometheus 指标。

基础备份能力还包括 SQLite 持久化队列、缺失分块续传、幂等资源提交、账户内内容去重、配额、Android WorkManager 和 iOS 后台 URLSession。手机本地删除默认不会删除服务器副本。

## 服务端启动

在 Debian/Ubuntu/WSL 安装 Rust 和编译依赖后：

```bash
sudo apt-get update
sudo apt-get install -y rustup pkg-config libssl-dev build-essential
rustup default stable
sudo ./scripts/setup-wsl.sh
cargo build --release -p photo-backup-server
sudo ./scripts/start-server-wsl.sh
```

服务启动时自动执行 SQLite 迁移。首次启动会把 `ADMIN_USERNAME` 和 `ADMIN_PASSWORD` 写入独立的管理员认证表；之后由数据库中的 Argon2 密码摘要完成登录。管理页面位于 `https://你的域名/admin`；管理员先创建普通用户，移动端随后用普通用户账号登录。开发机只读健康检查为 `http://127.0.0.1:8080/health`。

一个最小 Caddy 配置如下：

```caddyfile
photos.example.com {
    reverse_proxy 127.0.0.1:8080
}
```

Caddy 会自动处理证书并传递 HTTPS 代理信息。必须把直接连接 Axum 的代理 IP/CIDR 精确写入 `TRUSTED_PROXY_CIDRS`，并让代理覆盖或追加 `X-Forwarded-For` 与 `X-Forwarded-Proto`。服务从 Axum 提供的真实 socket peer 开始，由右向左逐跳解析代理链；未受信 peer 的所有转发头都会被忽略。仍应通过防火墙或回环绑定阻止绕过代理。

主要环境变量见 [.env.example](.env.example)：

- `DATABASE_URL`：SQLite 连接串，例如 `sqlite:///var/lib/isarmg/photo-backup/db/app.db`。
- `DATA_DIR`：原始媒体和临时分块根目录。
- `BIND`：推荐 `127.0.0.1:8080`。
- `REQUIRE_HTTPS`：生产必须保持 `true`。
- `DEVELOPMENT`：仅回环开发环境可设为 `true`，默认 `false`。
- `TRUSTED_PROXY_CIDRS`：逗号分隔的直接可信代理 IP/CIDR；为空时不信任任何转发头。示例中的 Caddy 与服务同机，因此只信任回环地址。
- `ADMIN_SESSION_IDLE_SECONDS` / `ADMIN_SESSION_ABSOLUTE_SECONDS`：管理员 Session 空闲/绝对期限，默认 1800/43200 秒。
- `METRICS_TOKEN`：启用 `/metrics` 的独立 Bearer Token；不设置则返回 404。

## Android 与 iOS

Android 工程在 `android/`，目标 API 36。先构建 Rust 动态库，再用 Android Studio 或 Gradle 构建：

```powershell
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
cargo install cargo-ndk
.\scripts\build-android-rust.ps1
```

iOS 工程描述在 `ios/project.yml`，需要 macOS、Xcode 和 XcodeGen：

```bash
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
./scripts/build-ios-rust.sh
cd ios && xcodegen generate
open PhotoBackup.xcodeproj
```

首次备份前选择要同步的相册。新设备只需登录同一账号；因为服务器保存原文件，不需要恢复密钥或设备配对。

## API 摘要

- 登录与上传：`POST /v1/auth/bootstrap`、`POST /v1/uploads`、`PUT /v1/uploads/{id}/parts/{index}`、`POST /v1/uploads/{id}/complete`。
- 恢复：`GET /v1/timeline`、`GET /v1/assets/{id}`、`GET /v1/resources/{id}/content`。
- 增量同步：`GET /v1/sync?after={sequence}`。
- 组织：`PATCH /v1/assets/{id}`、`GET|POST /v1/albums`、`GET|POST /v1/tags`、`POST|DELETE /v1/tags/{tag}/assets/{asset}`。
- 删除：`POST /v1/assets/{id}/trash`、`POST /v1/assets/{id}/restore`、`DELETE /v1/assets/{id}`。
- 重复项：`GET /v1/duplicates`；结合回收站接口处理多余副本。
- 自动化：`GET|POST /v1/api-keys`、`DELETE /v1/api-keys/{id}`、`GET /v1/audit-events`。
- 监控：`GET /metrics`，使用 `METRICS_TOKEN` 而不是用户或 API Key。

`GET /v1/timeline` 支持 `cursor`、`limit`、`trashed`、`favorite`、`archived`、`album_id` 和 `tag_id`。API Key 明文只在创建响应中出现一次。

## 从旧加密格式迁移

迁移会把既有对象标记为 `legacy-e2ee-v1`，新对象使用 `plain-v1`。服务器没有旧客户端的主密钥，因此不能自动把旧对象转换成原文件：必须用仍持有旧密钥的客户端下载解密，再用当前客户端重新上传。迁移会将未完成的旧加密上传标记失败，客户端会重新准备原始文件。

在确认所有旧对象已重传且可恢复之前，不要删除旧客户端、旧密钥或旧数据卷备份。

## 运维与发布

- PostgreSQL 与 `DATA_DIR` 必须作为一个一致性单元备份，并遵循至少 3-2-1 策略。
- 指标只提供聚合数量和字节数；审计 API 按序列分页返回设备/API Key 的关键操作。
- 大规模部署可在 `LocalStorage` 边界后增加 S3/MinIO，但不得改变 `plain-v1` 表示的“原始未加密字节”语义。
- 推送会运行 Rust、Android 和 iOS CI；版本标签 `vX.Y.Z` 必须与 Cargo 工作区版本一致。

## 许可证与参考边界

本项目第一方代码、文档和资源采用 [Apache License 2.0](LICENSE)。实现仅参考 Immich 的产品行为与公开架构概念，没有复制其代码、资源或迁移；被 `.gitignore` 排除的 `reference-immich/` 是独立 AGPL-3.0 参考副本，不属于本项目的 Apache-2.0 授权范围。
