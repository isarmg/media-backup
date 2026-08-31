# Media Backup 照片与视频备份

Media Backup `0.2.0` 是一个自托管照片与视频备份系统，由 Rust 服务端、Rust 共享核心、Android
客户端和 iOS 客户端组成。移动端通过 HTTPS 上传设备原始媒体和设备生成的缩略图；服务端使用
SQLite 保存账户、图库组织和同步状态，并以 `plain-v1` 保存与设备原文件一致的未加密字节。

本项目只实现当前版本。服务端只创建并接受 Media Backup `0.2.0` 当前 Schema，移动端只接受
`media-backup-mobile-v0.2-r1` 合约；产品代码不读取旧数据库、旧凭据或旧队列，也不提供迁移、备份
和恢复命令。这些离线任务统一由独立的 `sarmg-upgrade` 项目负责。

## 组成

```text
crates/server       Rust/Axum 服务端、管理页和运维命令
crates/protocol     服务端与移动端共享的数据协议
crates/crypto       Hash、密钥和安全比较基础能力
crates/agent-core   移动端本地队列与当前 SQLite 契约
crates/mobile-ffi   Android JNI 与 iOS C ABI 边界
clients/android/            Kotlin、Jetpack Compose、WorkManager 客户端
clients/ios/                SwiftUI、PhotoKit、后台 URLSession 客户端
clients/web/                编译进 Server 的管理客户端源码
config/                     可提交配置样例；生产 Secret 位于源码树外
scripts/            构建、发行、部署和契约门禁
```

## 快速验证

```bash
./scripts/check-mobile-v02-contract.sh
./scripts/check-workflow-supply-chain.sh
cargo fmt --all -- --check
cargo check --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

服务端开发构建可以在显式回环开发配置下运行；正式环境必须使用不可变发行归档，并由 Caddy 或
Nginx 在可信代理边界终止 TLS。完整步骤见运维文档。

## 文档

- [文档总览](docs/README.md)
- [初学者学习指南](docs/beginner-guide/README.md)
- [项目工作流程与流程树](docs/project-workflow.md)
- [完整功能与取舍清单](docs/feature-inventory-and-tradeoffs.md)
- [部署、诊断、安全与发布运维](docs/operations.md)

## 许可证

第一方代码、文档和资源采用 [Apache License 2.0](LICENSE)。项目只参考其他照片管理产品的公开行为
和架构思想，不复制其代码、资源、数据库结构或生成物。
