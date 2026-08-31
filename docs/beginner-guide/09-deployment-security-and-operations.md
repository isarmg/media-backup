# 第 9 章：部署、安全与生产运维

## 生产信任边界

Internet 只到 TLS reverse proxy；Axum 回环监听。Proxy、应用、SQLite、DATA_DIR 和移动 Secret 各自是
独立边界。`TRUSTED_PROXY_CIDRS` 只列直接 peer，不列所有互联网客户端。

## 不可变发行

正式归档由干净精确 tag 构建。binary 在打开配置/状态前验证自己的物理目录、source revision、target、
API/Schema/mobile epoch/Web 和全部文件 Hash/mode。安装只创建缺失的 `releases/0.2.0`，不覆盖同版本，
无 `current` symlink。

Server 的开发、测试和正式目标都只有 `x86_64-unknown-linux-gnu`。`sarmg-server-target=0.3.0` 在 crate
编译期执行共享 compile gate，Server 自身 build.rs 再无条件核对 Cargo `TARGET`。CI 必须显式
`--target`；build script 校验 x86_64 ELF；
安装/启动脚本、`serve-release` 的内核 uname 检查和 systemd `ConditionArchitecture=x86-64` 都拒绝其他
主机。移动端的 Android/iOS ARM target 不代表 Server 支持 ARM。

## 权限

release root-owned read-only；服务账户 `isarmg-media` 不可登录，仅写状态和 runtime。环境文件 0600；
数据库和媒体目录 0700/最小 umask。TLS 私钥由 proxy 账户保护，不授予应用读取。

## 上线检查

1. 校验外层 SHA256SUMS、binary identity 和 release verify。
2. 审核环境，无初始化 Secret 标记。
3. 检查 proxy/防火墙，后端不可被公网直连。
4. 启动后检查 liveness/readiness 和 Journal。
5. 管理员登录、创建测试用户、上传小媒体、下载和恢复。
6. 检查 metrics、磁盘/inode、SQLite WAL 和审计。

## 日常容量

媒体增长、缩略图、staging 与数据库分别预测。告警阈值要给最大并发 upload 留余量。磁盘不足时先停止
新上传，不直接删除 staging；根据 durable 状态执行受控清理。

## 备份恢复

停止服务或使用 `sarmg-upgrade` 支持的精确一致性方式，将 SQLite generation 与 DATA_DIR 一起备份。
原始媒体敏感，备份加密、异地、不可变并定期恢复演练。恢复先在隔离实例 `doctor` 和内容抽样，再切流。

## Secret 轮换

管理员密码、设备 Token、API Key、metrics Token、TLS key 分别轮换。认证器只接受当前有效摘要，不保留
双密钥 fallback；涉及持久数据代际的转换使用升级工具。轮换后验证被替换的凭据确实失败。

## 事件响应

先限制入口和写入，保全 release SHA、Journal、审计、数据库/data snapshot 与 proxy 日志，再轮换可能泄露
的凭据。公开 issue 不包含媒体、账号、Token、内部路径或数据库。安全修复只面向当前版本。

## 退役

停止并禁用服务，验证保留/销毁策略，使用升级工具导出所需当前备份，撤销所有设备/API Key，删除 DNS/
证书/代理入口，最后按审批销毁数据和 Secret。不能把卸载二进制等同于销毁用户媒体。
