# 第 1 章：项目全景与版本边界

## 学习目标

读完本章后，你应能说明 Media Backup 解决的问题、五个 Rust crate 与两个移动客户端的职责、媒体字节
经过哪些边界，以及为什么当前版本拒绝任何旧名称和旧状态。

## 产品目标

Media Backup 面向“把 Android/iOS 系统照片库中的原始照片和视频可靠保存到自己的服务器，并能在新
设备恢复”这一单一目标。核心成功标准不是页面数量，而是：扫描不越权、任务可续传、提交幂等、对象
可校验、恢复拿到原始字节、故障可诊断。

它不是完整媒体云。当前没有人脸、语义搜索、地图、分享、协作、服务端转码或机器学习集群。功能边界
详见[功能与取舍](../feature-inventory-and-tradeoffs.md)。

## 系统组成

```text
Android App                  iOS App
MediaStore                   PhotoKit
WorkManager                  Background URLSession
Keystore                     Keychain
      \                      /
       mobile-ffi + agent-core
                 |
              HTTPS /v2
                 |
       media-backup-server
       ├─ admin Web
       ├─ SQLite metadata
       └─ DATA_DIR blobs
```

`protocol` 定义 wire DTO；`crypto` 定义内容 Hash 等基础能力；`agent-core` 定义移动任务库；`mobile-ffi`
只做语言边界；`server` 拥有账户、上传、图库、文件和运维命令。

## 两类持久状态

- 服务端 SQLite：账户、设备、上传、资源、资产、相册、标签、同步序列、会话、API Key 和审计。
- 服务端 `DATA_DIR`：原始媒体、缩略图与暂存分块。
- 移动 SQLite：待处理任务、已上传分块、失败与游标。
- 移动安全存储：服务地址与设备 Bearer Token。

服务端数据库与 `DATA_DIR` 是一个组合一致性单元。只备份其中一项无法证明可恢复。

## 当前版本身份

当前身份由以下值共同组成：产品 `media-backup`、版本 `0.2.1`、移动 HTTP `/v2`、浏览器管理
`/api/v2`、存储 `plain-v1`、移动
epoch `media-backup-mobile-v0.2-r2`、两个精确 Schema、FFI header fingerprint、Web fingerprint、source
revision 和 release tree manifest。任一不匹配都失败关闭。

项目不注册旧项目名、不识别旧应用 ID、不扫描旧 state directory，也不为旧 JSON 字段添加 alias。
历史转换属于 `sarmg-upgrade`。

## 推荐源码阅读入口

先读 `Cargo.toml` 和各 crate manifest，再读 `crates/protocol/src/lib.rs`、`crates/server/src/main.rs`、
`routes.rs`、`agent-core/src/lib.rs`。随后分别进入 Android `BackupWorker.kt` 与 iOS
`BackupCoordinator.swift`。不要一开始逐行阅读所有 handler；先建立端到端流向。

## 本章检查题

1. 为什么 TLS 不等于端到端加密？
2. 为什么 SQLite 与 DATA_DIR 必须一起备份？
3. 为什么移动端安全存储不由 Rust FFI 直接拥有？
4. 哪些身份字段共同阻止旧版本被当前产品误读？
