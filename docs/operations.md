# Media Backup 运维文档

## 1. 生产拓扑与前置条件

推荐 Linux x86_64、systemd、Python 3、GNU coreutils/tar，以及同机 Caddy/Nginx：

```text
Internet -> HTTPS reverse proxy -> 127.0.0.1:8080 Media Backup
                                      ├─ SQLite /var/lib/isarmg/media-backup/db/app.db
                                      └─ media  /var/lib/isarmg/media-backup/data
```

媒体保存为明文字节，必须启用主机/卷加密、最小权限和加密异地备份。

## 2. 构建与验证发行归档

维护者从干净的 `0.2.0` checkout 构建：

```bash
revision="$(git rev-parse HEAD)"
MEDIA_BACKUP_SOURCE_REVISION="$revision" cargo build --release --locked -p media-backup-server
mkdir -p "$PWD/dist"
./scripts/build-server-release.sh "$PWD/target/release/media-backup-server" "$revision" "$PWD/dist"
./scripts/test-deployment.sh "$PWD/dist/media-backup-server-0.2.0-x86_64-unknown-linux-gnu.tar.gz"
```

构建器拒绝覆盖输出。归档 manifest 固定产品、版本、40 位 revision、target、`v2` API、`plain-v1`、
Schema、移动 FFI、Web 与全树文件权限/大小/SHA-256；额外文件、链接、特殊文件或硬链接别名均失败。

## 3. 安装

```bash
grep ' media-backup-server-0.2.0-x86_64-unknown-linux-gnu.tar.gz$' SHA256SUMS \
  | sha256sum --check -
tar -xzf media-backup-server-0.2.0-x86_64-unknown-linux-gnu.tar.gz
cd media-backup-server-0.2.0-x86_64-unknown-linux-gnu
./bin/media-backup-server release-identity
./bin/media-backup-server release-verify "$PWD"
sudo ./scripts/setup-wsl.sh
sudoedit /etc/isarmg/media-backup.env
sudo /opt/isarmg/media-backup/releases/0.2.0/scripts/start-server-wsl.sh
```

安装只允许创建缺失的 `/opt/isarmg/media-backup/releases/0.2.0`，不会覆盖或复用。同版本重装应先按
运维变更流程处理现有部署，而不是绕过 no-clobber。环境文件首次以 `0600` 排他创建；替换自动生成的
`ADMIN_PASSWORD`、`METRICS_TOKEN` 并删除初始化标记后才能启动。

## 4. 核心配置

| 变量 | 作用 | 生产要求 |
|---|---|---|
| `DATABASE_URL` | 当前 SQLite 路径 | 与媒体目录分离，路径父链不可是链接 |
| `DATA_DIR` | 原始媒体、缩略图和临时分块根 | 独立容量与 inode 监控 |
| `BIND` | HTTP 监听地址 | 推荐 `127.0.0.1:8080` |
| `REQUIRE_HTTPS` | 强制可信 HTTPS 语义 | 必须为 `true` |
| `DEVELOPMENT` | 本机开发开关 | 生产必须为 `false` |
| `TRUSTED_PROXY_CIDRS` | 直接可信代理地址 | 仅列真实直连代理 |
| `ADMIN_SESSION_*` | 管理会话期限 | 默认空闲 1800、绝对 43200 秒 |
| `METRICS_TOKEN` | `/metrics` 独立凭据 | 由秘密管理器生成和轮换 |

最小 Caddy 配置：

```caddyfile
photos.example.com {
    reverse_proxy 127.0.0.1:8080
}
```

防火墙必须阻止客户端绕过代理直连 Axum。代理应覆盖来源头；服务从真实 socket peer 开始由右向左
解析，未受信 peer 提供的转发头会被忽略。

## 5. 日常检查

```bash
curl --fail http://127.0.0.1:8080/health
curl --fail http://127.0.0.1:8080/health/ready
sudo /opt/isarmg/media-backup/releases/0.2.0/scripts/run-server-wsl.sh
```

带环境配置运行：

```bash
media-backup-server doctor
```

`doctor` 检查当前元数据和 Schema 指纹、`integrity_check`、`foreign_key_check`、对象 Hash、上传恢复
状态、可回滚数据库写探针与可清理存储探针。指标只暴露聚合数量和字节数，使用独立 Bearer Token。

## 6. 当前数据库合同

服务端 `product_metadata` 必须精确为 `application=media-backup`、`application_version=0.2.0`、
`schema_revision=1`，Schema SHA-256 为
`a464584cf7a55f9e50cb85bb539b1f42a9285f707440bb0bcfcd31a6b3a083c0`。移动队列对应
`media-backup-agent` 与 SHA-256
`fb38736bbf8ac69eb694095e62302f73233e39df42cd2d38e3dd1284e2f02558`。

数据库只在主文件不存在时创建。已存在空文件、未知元数据、旧版本或结构漂移会在业务写入前拒绝，
不能现场手改指纹“修复”。

## 7. 备份、恢复与升级

Media Backup 二进制不提供相关命令。停止服务后，由 `sarmg-upgrade` 把 SQLite 主文件及 sidecar 与
`DATA_DIR` 作为同一一致性单元处理。遵循 3-2-1 策略，备份加密并定期在隔离环境执行完整恢复演练。
恢复后先运行离线验证和 `doctor`，再开放流量。

## 8. 移动端构建

Android：

```powershell
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
cargo install cargo-ndk
.\scripts\build-android-rust.ps1
gradle -p android testDebugUnitTest assembleDebug
```

iOS：

```bash
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
./scripts/build-ios-rust.sh
cd ios && xcodegen generate
xcodebuild -project MediaBackup.xcodeproj -scheme MediaBackup -sdk iphonesimulator CODE_SIGNING_ALLOWED=NO build
```

## 9. 故障定位顺序

1. 检查固定发行树、manifest 和进程命令是否正确。
2. 检查 `/health/ready`、Journal 和磁盘/inode 容量。
3. 检查代理真实 peer、TLS、`TRUSTED_PROXY_CIDRS` 和客户端时间。
4. 运行 `doctor`，区分数据库合同、文件系统、Hash 或上传恢复错误。
5. 移动端检查系统权限、后台任务限制、本地队列和安全凭据存储。
6. 若是版本/Schema 问题，停止服务并转交 `sarmg-upgrade`，不要加入兼容代码。

## 10. 安全事件

不要在公开 issue 中附带生产数据库、媒体、密码、Token 或日志中的私人路径。先隔离入口、保全只读
证据和摘要，再轮换管理员密码、设备 Token、API Key、指标 Token、TLS 私钥及可能泄露的主机凭据。
安全修复只面向当前版本。
