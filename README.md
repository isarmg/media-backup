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
5. 时间线使用不透明游标分页；`/v2/sync` 提供单调序列的增量变更流，两端持久化同步游标。
6. 收藏、归档和标签已进入 SQLite 模型、筛选 API 和移动端操作；标签支持创建、批量设置、单项添加/移除。
7. 软删除进入回收站，可撤销；只有已进入回收站的资产才能永久删除。
8. 服务端按原始内容 BLAKE3/大小列出重复组，移动端显示重复组数量，可将多余副本移入回收站。
9. API Key 支持创建、列出和撤销；关键写操作进入审计日志；带独立令牌的 `/metrics` 输出 Prometheus 指标。

基础备份能力还包括 SQLite 持久化队列、缺失分块续传、幂等资源提交、账户内内容去重、配额、Android WorkManager 和 iOS 后台 URLSession。手机本地删除默认不会删除服务器副本。

## 服务端启动

在 Debian/Ubuntu/WSL 安装 Rust 和编译依赖后，先构建带锁定依赖的 release 制品，再安装：

```bash
sudo apt-get update
sudo apt-get install -y rustup pkg-config libssl-dev build-essential
rustup default stable
cargo build --release --locked -p photo-backup-server
sudo ./scripts/setup-wsl.sh
sudoedit /etc/isarmg/photo-backup.env
sudo ./scripts/start-server-wsl.sh
```

`setup-wsl.sh` 从 Cargo workspace 读取并校验 `MAJOR.MINOR.PATCH` 版本，把制品安装到不可变的 `/opt/isarmg/photo-backup/releases/<version>/bin`，再通过同目录临时链接和原子重命名切换 `/opt/isarmg/photo-backup/current`。同版本重复安装内容相同时是幂等的；内容不同会直接失败，不会覆盖既有制品。安装器只接受自己管理的 `current` 符号链接，其他安装目标及其父链出现符号链接或特殊文件时都会拒绝。

首次安装会以排他创建方式生成 `/etc/isarmg/photo-backup.env`，权限为 `0600`；重复安装只修正权限，不改配置内容。脚本不会把密码或 Token 写入源码树、命令行或日志，也不会启动服务。必须先用 `sudoedit` 替换自动生成且未回显的 `ADMIN_PASSWORD`、`METRICS_TOKEN`，删除 `# INITIAL-SECRETS-MUST-BE-REPLACED` 标记，再手动运行启动脚本。服务以不可登录的 `isarmg-photo` 用户和组运行，不使用 root 或 PostgreSQL；SQLite 位于 `/var/lib/isarmg/photo-backup/db/app.db`，媒体位于与 `db/` 分离的 `/var/lib/isarmg/photo-backup/data/`。systemd 使用严格只读系统视图，仅放行状态和运行目录写入。

服务仅在 SQLite 主文件不存在时以 `0600` 创建 Photo Backup 0.2.0 当前 Schema，不在产品进程内升级旧库。首次启动会把 `ADMIN_USERNAME` 和 `ADMIN_PASSWORD` 写入独立的管理员认证表；之后由数据库中的 Argon2 密码摘要完成登录。管理页面位于 `https://你的域名/admin`；管理员先创建普通用户，移动端随后用普通用户账号登录。开发机只读健康检查为 `http://127.0.0.1:8080/health`。`sudo ./scripts/run-server-wsl.sh` 可启动已安装的同一 systemd 服务并跟随 Journal 日志；它和启动脚本都不会读取或执行源码树 `.env`。

一个最小 Caddy 配置如下：

```caddyfile
photos.example.com {
    reverse_proxy 127.0.0.1:8080
}
```

Caddy 会自动处理证书并传递 HTTPS 代理信息。必须把直接连接 Axum 的代理 IP/CIDR 精确写入 `TRUSTED_PROXY_CIDRS`，并让代理覆盖或追加 `X-Forwarded-For` 与 `X-Forwarded-Proto`。服务从 Axum 提供的真实 socket peer 开始，由右向左逐跳解析代理链；未受信 peer 的所有转发头都会被忽略。仍应通过防火墙或回环绑定阻止绕过代理。

开发环境变量示例见 [.env.example](.env.example)；生产值只保存在 `/etc/isarmg/photo-backup.env`：

- `DATABASE_URL`：SQLite 连接串，例如 `sqlite:///var/lib/isarmg/photo-backup/db/app.db`。
- `DATA_DIR`：原始媒体和临时分块根目录。
- `BIND`：推荐 `127.0.0.1:8080`。
- `REQUIRE_HTTPS`：生产必须保持 `true`。
- `DEVELOPMENT`：仅回环开发环境可设为 `true`，默认 `false`。
- `TRUSTED_PROXY_CIDRS`：逗号分隔的直接可信代理 IP/CIDR；为空时不信任任何转发头。示例中的 Caddy 与服务同机，因此只信任回环地址。
- `ADMIN_SESSION_IDLE_SECONDS` / `ADMIN_SESSION_ABSOLUTE_SECONDS`：管理员 Session 空闲/绝对期限，默认 1800/43200 秒。
- `METRICS_TOKEN`：启用 `/metrics` 的独立 Bearer Token；不设置则返回 404。

## 当前 Schema 与诊断

服务器和 `doctor` 会按稳定顺序同时取得 SQLite 文件与 `DATA_DIR` 各自相邻的独占运行锁。如果另一进程复用任一数据库或数据目录，新进程会直接失败；锁目录的任何符号链接组件也会被拒绝。

```bash
photo-backup-server doctor
```

当前服务库必须含且只含一行 `product_metadata`：`application='photo-backup'`、`application_version='0.2.0'`、`schema_revision=1`、`schema_sha256='57c9282c425d2fe1baab63bfce2fa9d947b26b5bf3367750b0308aa442ccba0a'`。移动端 agent 队列库使用同一元数据表契约，值为 `application='photo-backup-agent'`、`application_version='0.2.0'`、`schema_revision=1`、`schema_sha256='fb38736bbf8ac69eb694095e62302f73233e39df42cd2d38e3dd1284e2f02558'`。实际指纹从 `sqlite_schema` 中排除 `sqlite_*` 与 `product_metadata`，按 `type,name,tbl_name` 排序；每行四个 UTF-8 字段依次写入八字节大端长度与原字节后计算 SHA-256。

服务库和 agent 队列库都只在目标路径不存在时初始化。已存在的空文件、无元数据库、0.1 数据库、其他产品/版本/修订，或任何实际 Schema 漂移都会在业务写入之前被拒绝。验证会把主文件、WAL 和 rollback journal 复制到私有临时代再打开，因此拒绝路径不会改动源库的主文件或 sidecar 字节；符号链接、硬链接别名和路径逃逸会 fail closed。

`doctor` 会检查精确元数据与 Schema 指纹、`integrity_check`、`foreign_key_check`、对象 Hash 和上传恢复状态，并执行可回滚的数据库写入探针及可清理的存储写读探针。产品二进制不再提供 schema migrate、database backup 或 restore 命令；这些工作必须由独立升级工具在服务停止后，将 SQLite 完整代与 `DATA_DIR` 作为同一一致性单元完成。不要绕过运行锁直接修改 SQLite 或 `DATA_DIR`。

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

当前网络协议固定为 `/v2`，不提供 `/v1` 兼容路由。上传请求必须显式携带 `storage_encoding: "plain-v1"`；服务端及当前 Android/iOS 客户端拒绝缺失、未知或其他存储协议值。

- 登录与上传：`POST /v2/auth/bootstrap`、`POST /v2/uploads`、`PUT /v2/uploads/{id}/parts/{index}`、`POST /v2/uploads/{id}/complete`。
- 恢复：`GET /v2/timeline`、`GET /v2/assets/{id}`、`GET /v2/resources/{id}/content`。
- 增量同步：`GET /v2/sync?after={sequence}`。
- 组织：`PATCH /v2/assets/{id}`、`GET|POST /v2/albums`、`GET|POST /v2/tags`、`POST|DELETE /v2/tags/{tag}/assets/{asset}`。
- 删除：`POST /v2/assets/{id}/trash`、`POST /v2/assets/{id}/restore`、`DELETE /v2/assets/{id}`。
- 重复项：`GET /v2/duplicates`；结合回收站接口处理多余副本。
- 自动化：`GET|POST /v2/api-keys`、`DELETE /v2/api-keys/{id}`、`GET /v2/audit-events`。
- 监控：`GET /metrics`，使用 `METRICS_TOKEN` 而不是用户或 API Key。

`GET /v2/timeline` 支持 `cursor`、`limit`、`trashed`、`favorite`、`archived`、`album_id` 和 `tag_id`。API Key 明文只在创建响应中出现一次。

## 运维与发布

- SQLite 完整代与 `DATA_DIR` 必须由独立升级/维护工具作为一个一致性单元备份，并遵循至少 3-2-1 策略和定期恢复演练。
- 指标只提供聚合数量和字节数；审计 API 按序列分页返回设备/API Key 的关键操作。
- 大规模部署可在 `LocalStorage` 边界后增加 S3/MinIO，但不得改变 `plain-v1` 表示的“原始未加密字节”语义。
- 推送会运行 Rust、Android 和 iOS CI；版本标签 `vX.Y.Z` 必须与 Cargo 工作区版本一致。

## 许可证与参考边界

本项目第一方代码、文档和资源采用 [Apache License 2.0](LICENSE)。实现仅参考 Immich 的产品行为与公开架构概念，没有复制其代码、资源或迁移；被 `.gitignore` 排除的 `reference-immich/` 是独立 AGPL-3.0 参考副本，不属于本项目的 Apache-2.0 授权范围。
