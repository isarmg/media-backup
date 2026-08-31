# 第 4 章：服务端请求生命周期

## 从 socket 到 handler

请求首先以连接扩展中的真实 socket peer 为信任起点。可信代理模块只有在直接 peer 属于配置 CIDR 时
才解释转发链，并从右向左去除可信 hop；否则忽略所有 forwarded header。随后执行 HTTPS policy、body
limit、路由匹配、身份提取、CSRF/权限、DTO 反序列化，最后进入业务 handler。当前没有请求 ID 中间件。

## 四种身份

1. 管理员浏览器：Secure Cookie Session + CSRF。
2. 移动设备：设备 Bearer Token。
3. 自动化：API Key Bearer Token。
4. 监控：独立 Metrics Bearer Token。

路由显式选择一种身份，不能“依次尝试”多种 Token。这样避免低权限凭据意外落入高权限解析路径。

管理员不是通用 RBAC 用户。`auth_users` 当前表只保存 canonical `username`，不保存 email 或恒等角色，
配置入口只有 `ADMIN_USERNAME`；wire Session 的 `role` 由服务端固定输出 `admin`。operator/viewer、
角色切换和 `ADMIN_EMAIL` 都不存在。浏览器登录、恢复会话、退出分别固定为
`POST /api/v2/auth/login`、`GET /api/v2/auth/session`、`POST /api/v2/auth/logout`，管理 handler 固定在
`/api/v2/admin/*`。移动端 `accounts.username` 与这套管理 username 完全隔离。

## 登录 admission

body 在 Argon2 之前限为 4 KiB。来源与规范化账户各有 bounded Token Bucket；未知/停用账户仍执行相同
参数 dummy verification。Argon2 由 semaphore 限制并发，并有独立超时。429 包含 Retry-After；错误不
暴露账户存在性。

canonical username（3–64 bytes、首尾字母数字、字符 `[a-z0-9._-]`）、唯一当前 Argon2id 参数、随机
Session/CSRF token、SHA-256 摘要/常量时间匹配和 raw
Cookie 解析均直接来自 `sarmg-admin-auth=0.3.0`。产品只在其外层保留 admission、数据库、Cookie flags、
TTL 和撤销；只接受当前 Hash policy，也不复制一套同形安全函数。

## 上传路由

移动合约的 product/version/revision 在 Agent 入队边界验证；HTTP create upload 验证 storage encoding、
配额、资源 metadata、manifest body、part 连续编号/size 与 BLAKE3。PUT part 最多等待 30 秒取得全局与
账户 admission permit，取得 permit 后流式执行 size/Hash 校验并写入唯一 staging；当前没有覆盖整个 body
传输的独立 wall-clock deadline。complete 先重新验证所有 durable record 与文件 identity，再合并、Hash、
发布对象并提交资产事务。

客户端断开不应让已经提交给持久状态机的任务变成无人拥有。相反，在进入 durable point 之前取消要
清理临时文件。

## 图库路由

时间线 cursor 是服务端编码的不透明值，限制 limit 和筛选组合。相册/标签 mutation 验证同账户 owner；
完整扫描与分批扫描使用不同完成语义。收藏、归档、回收站改变时追加 sync sequence 和 audit。

永久删除先确认 trashed；asset/resource 删除提交后，无引用 blob 行保留为可重试的物理回收意图。rooted
unlink 与 blob-row 删除在同一 SQLite 事务中收口，失败回滚并由协调器重试；204 表示已收口，202 表示
用户可见删除已提交但物理回收仍待处理。仍有账户内资源引用的 blob 不会进入回收；本项目不跨账户去重。

## 响应与日志

成功 JSON 使用当前 DTO；`AppError` 统一映射为顶层 Foundation `code/message/retryable` ErrorEnvelope；
当前实现不生成 request ID，因此不伪造该可选字段。TraceLayer 记录 HTTP span，关键认证、提交协调和
存储错误另写结构化事件；不得假定每条日志都有统一操作 ID 或耗时字段。日志不应包含密码、Token、
媒体字节或任意客户端路径。metrics 只输出固定聚合 gauge，不使用 username 等高基数标签。

## 请求失败分层

- 400/422：请求形状或业务约束，通常不可原样重试。
- 401/403：身份/权限；设备需重新 bootstrap 或由操作者更新当前凭据，不能凭空刷新 Token。
- 409：幂等 identity 或状态冲突，先读取权威状态。
- 429：等待 Retry-After。
- 5xx：服务暂不可用，采用有界退避并保留本地队列。
