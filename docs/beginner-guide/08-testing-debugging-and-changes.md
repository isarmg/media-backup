# 第 8 章：测试、调试与变更方法

## 测试层级

- Rust unit：纯 DTO、Hash、状态机、路径、限流。
- Rust integration：真实 SQLite、WAL、HTTP router、文件系统与并发。
- 契约静态门：Rust FFI header、JNI、Swift symbol、版本/application ID。
- Android：Kotlin unit、Compose/Gradle compile、APK assemble。
- iOS：XcodeGen、epoch isolation test、simulator compile。
- 发行：真实 source-bound binary、archive、install、relocation、tamper negative cases。

## 本地质量门

```bash
./scripts/check-mobile-v02-contract.sh
./scripts/check-workflow-supply-chain.sh
cargo fmt --all -- --check
cargo check --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

只跑目标测试适合迭代，不是最终验收。涉及登录 Argon2 的测试对 CPU 争用敏感；并发跑多个大 workspace
可能制造 timeout 假失败，应在资源正常时单独复现。

## 调试分层

先记录 request ID、设备 job ID、upload ID、asset/resource ID 和时间，再定位层次：扫描权限 -> 本地队列
-> DNS/TLS -> auth -> create -> part -> complete -> sync -> restore。不要从 UI “失败”直接猜数据库。

## SQLite 调试

优先运行 `doctor` 和测试 fixture，不在生产库运行手工 DDL。要分析拒绝路径，复制完整 generation 到
隔离目录并保持原 mode/sidecar。确认错误是 identity、Schema、integrity、foreign key、锁还是存储。

## 文件系统调试

核对 mount、free bytes、inode、owner/mode、real path、链接和 open file。遇到 unknown staging 不按名称
删除；先对应数据库 upload record 与 Hash。

## 安全变更检查表

1. 外部输入是否有长度/数量/depth 上限？
2. Secret 是否可能进入日志、panic 或 metrics label？
3. 取消/timeout 后谁拥有工作？
4. crash 在每个 durable point 后如何恢复？
5. 第二实例或路径换绑能否绕过锁？
6. 是否无意加入旧名称/旧 Schema 兼容？

## 提交与评审

一个大问题一个提交：协议、存储、平台 UI、发行/运维分别可回滚。提交前查看 `git diff --check`、旧名
称零残留、生成物未提交和文档链接。评审描述不只写“测试通过”，还列出失败语义和未覆盖平台。
