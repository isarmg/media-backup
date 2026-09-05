# Media Backup 初学者学习指南

本手册按 Dufs 教学文档相同的十章结构，从产品心智模型逐步深入到协议、状态机、移动平台、测试和生产
运维。建议按顺序阅读；已有经验的维护者可按主题跳转，但处理版本或安全问题时仍应核对前五章。

1. [项目全景与版本边界](01-project-overview.md)
2. [开发环境与第一次运行](02-environment-and-first-run.md)
3. [Rust、移动宿主与 HTTP 基础](03-rust-mobile-and-http-basics.md)
4. [服务端请求生命周期](04-server-request-lifecycle.md)
5. [存储、状态机与可靠性](05-storage-state-and-reliability.md)
6. [Android 与 iOS 客户端](06-android-and-ios-clients.md)
7. [当前协议、上传与同步](07-current-protocol.md)
8. [测试、调试与变更方法](08-testing-debugging-and-changes.md)
9. [部署、安全与生产运维](09-deployment-security-and-operations.md)
10. [源码路线、练习与术语表](10-reading-roadmap-and-glossary.md)

下文保留一份单页快速导读；详细解释、练习和失败语义以上述章节为准。

## 1. 先理解产品边界

Media Backup 解决的是“手机原始照片和视频可靠上传到自有服务器，并可在新设备恢复”的问题。它不是
完整的社交图库，也不是零知识加密系统。TLS 保护网络传输，服务端最终保存原始明文字节，因此服务器
和数据卷管理员能够读取媒体；生产安全依赖主机权限、磁盘加密、TLS 和可靠备份。

项目坚持当前版本边界：`0.2.0` 服务端、移动 `/v2` API、浏览器管理 `/api/v2` API、`plain-v1` 存储和
`media-backup-mobile-v0.2-r1` 移动合约构成一个不可拆分的版本身份。发现非当前状态时产品必须只读拒绝，
不能猜测或转换。

## 2. 认识四层架构

```text
系统照片库
  └─ Android MediaStore / iOS PhotoKit
       └─ 原生 UI、权限、后台调度、安全凭据存储
            └─ mobile-ffi + agent-core 本地队列
                 └─ HTTPS /v2 API
                      └─ Axum 服务端
                           ├─ SQLite 元数据
                           └─ DATA_DIR 原始媒体与缩略图
```

- 原生层拥有相册权限、系统后台任务和 Keychain/Keystore。
- Rust 移动核心拥有严格 JSON 合约、SQLite 队列、分块状态和幂等提交逻辑。
- 服务端拥有身份、配额、图库组织、审计、对象存储和当前 Schema。
- 反向代理拥有公网 TLS；服务端默认只应监听回环地址。

## 3. 准备开发环境

基础服务端开发需要 Rust 与 Cargo：

```bash
rustup show
cargo check --workspace --locked
cargo test --workspace --locked
```

Android 还需要 JDK、Android SDK API 36、NDK `28.2.13676358` 和 `cargo-ndk`。iOS 需要 macOS、
Xcode、XcodeGen，以及 `aarch64-apple-ios`、`aarch64-apple-ios-sim` Rust target。具体构建命令见
[运维文档](../operations.md)。

不要把生产密码写进 `.env` 后提交。`config/media-backup.env.example` 只说明字段；生产配置位于源码树外。

## 4. 从代码入口开始阅读

推荐顺序：

1. `crates/protocol/src/lib.rs`：先看网络对象和版本字段。
2. `crates/server/src/main.rs` 与 `routes.rs`：理解命令入口、移动 `/v2` 与浏览器 `/api/v2` 路由。
3. `admin.rs`、`auth.rs`、`login_admission.rs`、`api_access.rs`、`metrics.rs`：理解 Administrator、device、
   API Key 与 metrics 四条身份边界。
4. `storage.rs`、`upload_commit.rs`、`rooted_fs.rs`：理解文件如何安全落盘。
5. `agent-core/src/database.rs`：理解移动队列和本地当前 Schema。
6. Android 的 `BackupWorker.kt`、iOS 的 `BackupCoordinator.swift`：理解宿主调度。
7. 两端 `RemoteLibrary`、相册扫描器与 UI：理解恢复和图库操作。

## 5. 完成一次开发启动

仅本机开发允许关闭 HTTPS 强制，并且必须显式满足 `DEVELOPMENT=true` 与回环 `BIND`。先复制
`config/media-backup.env.example` 中需要的值到终端环境，再运行未绑定源码 revision 的开发二进制：

```bash
cargo run -p media-backup-server -- serve
```

然后检查：

```bash
curl --fail http://127.0.0.1:8080/healthz
```

正式源码绑定二进制拒绝普通 `serve`，只能通过已验证发行树执行 `serve-release`。

## 6. 理解一次上传

扫描器只读取用户授权的相册，计算原始内容 BLAKE3，并生成缩略图。任务写入本地 SQLite 后才进入
后台上传。客户端先创建上传会话，再查询缺失分块，只发送缺失内容；服务端对每块和最终对象校验，
最后以幂等事务提交资产、资源和同步序列。应用被系统终止后，持久化队列允许下一次调度继续。

关键语义：手机本地删除默认不会删除服务端副本；永久删除只允许作用于已经进入回收站的资产。

## 7. 理解认证

- 浏览器管理员使用 `__Host-sarmg-media-backup-session` Cookie 与 Session 绑定 CSRF。
- 移动设备使用 bootstrap 后取得的 Bearer Token。
- 自动化使用可撤销 API Key。
- `/metrics` 使用独立 `METRICS_TOKEN`。

这些凭据不能互换。密码使用 Argon2；服务端只保存设备 Token 和 API Key 的 SHA-256 摘要。登录路径
同时受请求体大小、真实来源、账户、Argon2 并发和超时限制。

## 8. 如何安全修改

修改协议时，同时更新 Rust DTO、Android/iOS 合约、FFI 头文件与契约测试；修改 Schema 时更新当前
Schema、元数据指纹和所有新库测试，不在产品里添加 migration；修改发行布局时更新 manifest 生成器、
运行时验证器和部署测试。完成修改后运行根 README 的完整门禁。

常见错误：只改一个移动端、为非当前 JSON 字段增加 alias、直接编辑既有数据库、让服务端读取
移动端 Keychain/Keystore、或者让反向代理来源头绕过真实 peer 校验。

## 9. 调试方法

先区分错误发生在权限扫描、本地队列、HTTPS、鉴权、上传分块、提交事务还是文件系统。当前日志没有
统一 request ID，应以时间、job/upload/asset/resource ID 和操作事件关联，同时不得记录密码、Token 或
媒体内容。数据库或存储异常先运行 `doctor`；
若它报告版本/Schema 不匹配，不要强行修表，应停止产品并使用独立升级工具。

## 10. 术语

- **BLAKE3**：本项目用于内容完整性和重复项分组的 Hash。
- **CSRF**：浏览器 Cookie 会话的跨站请求伪造防护。
- **FFI/JNI**：Rust 与 Swift/Kotlin 之间的调用边界。
- **WAL**：SQLite Write-Ahead Log；数据库一致性副本必须考虑其 sidecar。
- **epoch**：不兼容合约代，本项目移动端当前值为 `media-backup-mobile-v0.2-r1`。
- **fail closed**：无法证明输入安全或身份精确匹配时拒绝处理。
