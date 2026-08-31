# 第 3 章：Rust、移动宿主与 HTTP 基础

## Rust workspace 的依赖方向

允许方向大致为 `protocol <- crypto <- agent-core/mobile-ffi`，服务端组合 protocol/crypto。共享 crate 不
应反向依赖 Android、iOS 或 server。循环依赖通常意味着职责放错。

Rust 中 `Result<T, E>` 表示可恢复失败；`?` 传播错误并保留 context。处理外部输入时不要 `unwrap`。
`serde(deny_unknown_fields)` 是当前 wire contract 的重要组成，不能为了“客户端方便”删除。

## 异步与阻塞工作

Axum handler 在 Tokio runtime 上运行。文件流和网络 I/O 使用 async；Argon2、部分 SQLite/文件验证等
阻塞工作必须进入受限阻塞池或专门并发闸门。把重 CPU 直接放 handler 会拖慢所有请求。

## Android 宿主职责

Kotlin 拥有权限、MediaStore cursor、Compose UI、WorkManager、网络环境和 EncryptedSharedPreferences/
Keystore。JNI 方法只接收严格 JSON 和路径，不应在 Rust 中偷偷请求 Android 权限或持有 Activity。

系统可能延迟、合并或取消后台任务，因此算法必须依赖持久队列和幂等服务端，而不是假设 Worker 永远
连续运行。

## iOS 宿主职责

Swift 拥有 PhotoKit authorization、SwiftUI 状态、Keychain、BGTask/URLSession delegate 与系统照片库
写入。FFI handle 的生命周期由 `RustAgent` 封装，C 字符串必须由匹配的 current free 函数释放。

Swift continuation 只能 resume 一次；后台 delegate、取消和进程恢复必须汇入同一状态机，不能在多个
回调重复提交。

## HTTP 约定

JSON 请求设置正确 Content-Type，并受 body 上限。错误使用稳定 HTTP status 与 Foundation
`code/message/retryable`；当前 Server 不生成 request ID，客户端也不能依赖该可选字段。展示 message
不用于程序分支。下载与分块上传是二进制流，不能先无界读入内存再检查大小。

`GET`/`HEAD` 不改变业务状态；使用 `EmptyRequest` 的 action POST/DELETE 要求精确 `{}`，以拒绝未审计
字段。登录、创建上传、PATCH 等路由则各自使用明确 DTO，不能把 `{}` 规则泛化到全部写请求。

## SQLite 基础

SQLite transaction 只能原子管理数据库，不能原子覆盖文件系统和移动系统照片库。项目通过 staged
file、Hash、rename、持久 upload record 和恢复扫描组合出可证明状态。WAL 使读写并发更好，也意味着
运维必须按完整 generation 处理 sidecar。

## 安全编码原则

- 外部长度先验证再分配。
- 路径按组件和已打开目录描述符约束，不用字符串前缀判断。
- Secret 使用专用类型/日志 redaction，不进入 error detail。
- Secret Token 的摘要与比较必须沿用既有认证边界；内容 BLAKE3 不是授权凭据。
- spawn 后的任务必须写明生命周期。当前周期协调任务由进程拥有，没有独立取消句柄或 shutdown join；
  不得在文档中把尚未实现的 graceful shutdown 当成保障。
