# Media Backup 运维文档

## 1. 生产拓扑与前置条件

正式 Server **只支持 Linux x86_64**，并要求 systemd、Python 3、GNU coreutils/tar，以及同机
Caddy/Nginx。`aarch64` 主机、非 Linux 主机和非 GNU Rust target 都会失败关闭，不存在交叉架构 fallback：

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
MEDIA_BACKUP_SOURCE_REVISION="$revision" cargo build --release --locked \
  -p media-backup-server --target x86_64-unknown-linux-gnu
mkdir -p "$PWD/dist"
./scripts/build-server-release.sh \
  "$PWD/target/x86_64-unknown-linux-gnu/release/media-backup-server" \
  "$revision" "$PWD/dist"
./scripts/test-deployment.sh "$PWD/dist/media-backup-server-0.2.0-x86_64-unknown-linux-gnu.tar.gz"
```

Cargo release build script 拒绝其他 target；归档脚本还会核对构建主机为 Linux x86_64，并直接检查输入
二进制为 64 位 little-endian x86_64 ELF。构建器拒绝覆盖输出。归档 manifest 固定产品、版本、40 位
revision、target、`v2` 移动 API、`plain-v1`、Schema、移动 FFI、Web 与全树文件权限/大小/SHA-256；
额外文件、链接、特殊文件或硬链接别名均失败。

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
`ADMIN_USERNAME`、`ADMIN_PASSWORD`、`METRICS_TOKEN` 并删除初始化标记后才能启动。登录候选 username
必须是 1–64 bytes 的可打印 ASCII；Foundation 会去除首尾 ASCII whitespace、转为 ASCII 小写，再要求
canonical 值为 3–64 bytes、首尾字母数字且全部字符仅为 `[a-z0-9._-]`，因此 `@`、Unicode、内部空白、
首尾分隔符都被拒绝。持久化和 Session 只接受已经 canonical 的值；`ADMIN_EMAIL` 不是配置别名。

## 4. 核心配置

| 变量 | 作用 | 生产要求 |
|---|---|---|
| `DATABASE_URL` | 当前 SQLite 路径 | 与媒体目录分离，路径父链不可是链接 |
| `DATA_DIR` | 原始媒体、缩略图和临时分块根 | 独立容量与 inode 监控 |
| `BIND` | HTTP 监听地址 | 推荐 `127.0.0.1:8080` |
| `ADMIN_USERNAME` | 唯一浏览器管理员 username | 必填；按 Foundation 规范化后必须为 canonical `[a-z0-9._-]`，当前库唯一 |
| `ADMIN_PASSWORD` | 唯一浏览器管理员密码 | Foundation 当前策略：12–1024 bytes、无 ASCII 控制字符；生产由秘密管理器生成 |
| `REQUIRE_HTTPS` | 强制可信 HTTPS 语义 | 必须为 `true` |
| `DEVELOPMENT` | 本机开发开关 | 生产必须为 `false` |
| `TRUSTED_PROXY_CIDRS` | 直接可信代理地址 | 仅列真实直连代理 |
| `ADMIN_SESSION_*` | 管理会话期限 | 默认空闲 1800、绝对 43200 秒 |
| `METRICS_TOKEN` | `/metrics` 独立凭据 | 由秘密管理器生成和轮换 |

浏览器认证合同只有三条：`POST /api/v2/auth/login`、`GET /api/v2/auth/session`、
`POST /api/v2/auth/logout`。登录 body 精确为 `{username,password}`；登录和 session 成功体精确为
`{authenticated:true,user_id,username,role:"admin",csrf_token}`。普通备份账户仍使用
`accounts.username`，它与 `auth_users.username` 是不同身份域；同名不会共享密码、Session、数据归属或
权限。用户管理等业务位于 `/api/v2/admin/*`，移动端仍只使用 `/v2/*`。管理员 username 规范化、严格
当前 Argon2id、随机 Session/CSRF token、摘要比较和原始 Cookie 解析来自
`sarmg-admin-auth=0.3.0`；本项目只拥有登录限流、Session 表、Cookie 属性/TTL 与撤销。

此合同调整的范围只有 Server 和编译进 Server 的 React/Vite 管理 Web。Android/iOS、Agent 数据库、
`/v2/auth/bootstrap`、备份账户 username、设备 Token 与 API Key 均保持当前移动合同，运维不得把
`ADMIN_USERNAME` 写入移动客户端配置，也不得把普通账户提升或复制到 `auth_users`。

最小 Caddy 配置：

```caddyfile
media.example.com {
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

启动脚本先检查 `uname`，二进制的 `serve-release` 再通过内核 `uname(2)` 检查 Linux x86_64，systemd 单元
还有 `ConditionArchitecture=x86-64`。三层任何一层不满足都必须在读取业务配置和创建状态前失败。

带环境配置运行：

```bash
media-backup-server doctor
```

`doctor` 检查当前元数据和 Schema 指纹、`integrity_check`、`foreign_key_check`、对象 Hash、上传恢复
状态、无引用 blob 回收意图、可回滚数据库写探针与可清理存储探针。若报告待回收 blob，先在服务停止
且同一环境配置下运行：

```bash
media-backup-server reconcile scan
media-backup-server doctor
```

`reconcile scan` 与服务启动/120 秒周期使用同一协调路径和运行锁；它会重试未完成 upload commit、无引用
blob rooted unlink/删行及 orphan commit staging 清理。永久删除响应 202 表示用户可见 metadata 已删除但
物理 blob 尚待该协调路径收口，不能盲目重放 DELETE；204 才表示本次已完成物理回收。指标只暴露聚合
数量和字节数，使用独立 Bearer Token。

## 6. 当前数据库合同

服务端 `product_metadata` 必须精确为 `application=media-backup`、`application_version=0.2.0`、
`schema_revision=1`，Schema SHA-256 为
`2563e6afc3fff272d02b7a5615272cc773862243bfd15aec51655abf1d9c6b1c`。移动队列对应
`media-backup-agent` 与 SHA-256
`fb38736bbf8ac69eb694095e62302f73233e39df42cd2d38e3dd1284e2f02558`。

数据库只在主文件不存在时创建。已存在空文件、非当前元数据或结构漂移会在业务写入前拒绝，不能
现场手改指纹“修复”。

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
gradle -p clients/android testDebugUnitTest assembleDebug
```

上述命令只生成不发布的 Debug APK，不得获得正式签名 Secret。正式 `v0.2.0` workflow 使用 application ID
`org.sarmg.mediabackup`、alias `media-backup-android-release` 和受保护 `android-signing` Environment 中仅有的
`MEDIA_BACKUP_ANDROID_SIGNING_PKCS12_BASE64`、`MEDIA_BACKUP_ANDROID_SIGNING_PKCS12_PASSWORD` 两个
Secret 执行 `assembleRelease`。证书必须是 RSA 4096、`CA:FALSE`、Digital Signature/Code Signing，SHA-256
必须精确为
`0C:FC:28:11:D4:8C:DE:AB:3E:6D:85:70:29:D8:79:E0:01:AB:95:31:C0:67:84:B4:D4:8D:15:A8:47:77:14:21`。
上传前 workflow 用 `apksigner` 验证唯一 signer，用 `aapt2` 验证 application ID，并确认 APK 只含
`arm64-v8a/libmedia_backup_mobile.so`。缺 Secret、错误 alias/密码/指纹、Debug APK 或其他 ABI 均失败关闭。
该全新身份不更新任何旧应用；旧签名、旧 package ID、旧 Secret 名和 fallback 均不进入当前仓库。

iOS：

```bash
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
./scripts/build-ios-rust.sh
cd clients/ios && xcodegen generate
xcodebuild -project MediaBackup.xcodeproj -scheme MediaBackup -sdk iphonesimulator CODE_SIGNING_ALLOWED=NO build
```

## 9. 故障定位顺序

1. 检查固定发行树、manifest 和进程命令是否正确。
2. 检查 `/health/ready`、Journal 和磁盘/inode 容量。
3. 检查代理真实 peer、TLS、`TRUSTED_PROXY_CIDRS` 和客户端时间。
4. 运行 `doctor`，区分数据库合同、文件系统、Hash 或上传恢复错误。
5. 移动端检查系统权限、后台任务限制、本地队列和安全凭据存储。
6. 若是版本/Schema 问题，停止服务并转交 `sarmg-upgrade`，不要加入兼容代码。

移动 Agent 的当前已知边界：`retry_wait` 到期会重新准备源文件，不会复用仍持久化的 `prepared_json`；
若扫描器已按 `remove_source_after_prepare` 删除导出临时源，上传失败后可能持续报源不存在。准备失败或
`preparing` 崩溃也可能留下 partial part。采集 job ID、数据库行和对应目录证据后再处置；不得删除整个
`backup-staging-v0.2-r1/prepared/`，也不得把数据库中未经校验的 ID 直接拼为递归删除目标。

## 10. 安全事件

不要在公开 issue 中附带生产数据库、媒体、密码、Token 或日志中的私人路径。先隔离入口、保全只读
证据和摘要，再轮换管理员密码、设备 Token、API Key、指标 Token、TLS 私钥及可能泄露的主机凭据。
安全修复只面向当前版本。

## 11. 管理 Web 与 Foundation 门禁

管理 Web 必须使用 `.node-version` 指定的 Node `26.7.0`。Foundation 是构建期依赖；生产机不安装 npm
包，不访问 Foundation 仓库、registry 或 CDN。`build` 自带 `check:foundation` 前置门禁，因此正式顺序为：

```bash
npm ci --prefix clients/web
npm run build --prefix clients/web
MEDIA_BACKUP_SOURCE_REVISION="$(git rev-parse HEAD)" \
  cargo build --release --locked -p media-backup-server \
  --target x86_64-unknown-linux-gnu
```

门禁直接调用 Foundation `assertAdministratorWebToolchain`，同时验证精确依赖/lockfile、三个共享 CSS
入口及摘要、React 根和 `data-sarmg-scope`。Vite 只生成 `dist/index.html`、`dist/assets/admin.js`、
`dist/assets/admin.css`；Rust 二进制与发行包 `share/web/` 都绑定这三个字节。Server 先于 Web 构建会因
缺少 include 目标而失败，这是防止源码/产物混代的预期行为。

当前四个包已直接固定到 Foundation GitHub Release `v0.3.0` 的
`sarmg-admin-web-0.3.0.tgz`、`sarmg-contracts-0.3.0.tgz`、`sarmg-design-tokens-0.3.0.tgz` 与
`sarmg-http-client-0.3.0.tgz`；`package-lock.json` 必须同时记录这些 URL、版本 `0.3.0` 和各归档的
`sha512` integrity。Rust Foundation crate 则同时固定 Git 仓库、版本 `=0.3.0` 与完整 revision
`1fe326081cfd896f05ff502e80f99504797c14c6`。独立 checkout/CI 不读取共同父目录或 sibling 源码。
门禁失败时修正依赖、lockfile 或产品源码，再完整重建二进制和发行归档；不得改回 `file:`/`path`、
在线编辑 `share/web`、复制旧 dist、vendoring 共享 CSS 或加入网络 fallback。
