# 第 4 章：服务端请求生命周期

## 从 socket 到 handler

请求首先取得真实 socket peer 和 request ID。可信代理模块只有在直接 peer 属于配置 CIDR 时才解释转发
链，并从右向左去除可信 hop；否则忽略所有 forwarded header。随后执行 HTTPS policy、body limit、路由
匹配、身份提取、CSRF/权限、DTO 反序列化，最后进入业务 handler。

## 四种身份

1. 管理员浏览器：Secure Cookie Session + CSRF。
2. 移动设备：设备 Bearer Token。
3. 自动化：API Key Bearer Token。
4. 监控：独立 Metrics Bearer Token。

路由显式选择一种身份，不能“依次尝试”多种 Token。这样避免低权限凭据意外落入高权限解析路径。

## 登录 admission

body 在 Argon2 之前限为 4 KiB。来源与规范化账户各有 bounded Token Bucket；未知/停用账户仍执行相同
参数 dummy verification。Argon2 由 semaphore 限制并发，并有独立超时。429 包含 Retry-After；错误不
暴露账户存在性。

## 上传路由

创建 upload 时验证 product/version/storage encoding、配额、资源 metadata、part size/count 与 BLAKE3。
PUT part 在读 body 过程中执行 deadline/size/Hash，写入唯一 staging。complete 先重新验证所有 durable
record 与文件 identity，再合并、Hash、发布对象并提交资产事务。

客户端断开不应让已经提交给持久状态机的任务变成无人拥有。相反，在进入 durable point 之前取消要
清理临时文件。

## 图库路由

时间线 cursor 是服务端编码的不透明值，限制 limit 和筛选组合。相册/标签 mutation 验证同账户 owner；
完整扫描与分批扫描使用不同完成语义。收藏、归档、回收站改变时追加 sync sequence 和 audit。

永久删除先确认 trashed，再检查对象是否仍被账户内其他资源引用；跨账户 dedupe 不能泄露存在性。

## 响应与日志

成功 JSON 使用当前 DTO；错误统一映射 code/status/request ID。日志包含操作类型、稳定 ID、耗时与结果，
不包含密码、Token、媒体字节或任意客户端路径。metrics 聚合计数，不使用用户 email 等高基数标签。

## 请求失败分层

- 400/422：请求形状或业务约束，通常不可原样重试。
- 401/403：身份/权限；设备需确认当前凭据，不循环刷新不存在的兼容 Token。
- 409：幂等 identity 或状态冲突，先读取权威状态。
- 429：等待 Retry-After。
- 5xx：服务暂不可用，采用有界退避并保留本地队列。
