# 第 7 章：当前协议、上传与同步

## 唯一 namespace

移动备份业务 API 只位于 `/v2`。浏览器管理认证只位于 `/api/v2/auth`，管理业务只位于
`/api/v2/admin`；`/admin` 是静态管理入口，health/metrics 是运维入口。没有 `/v1`、旧的
`/v2/admin/*`、未版本化、旧项目名、尾斜杠 alias 或 fallback redirect。

## 浏览器管理员合同

| 方法与路径 | 精确请求 | 成功响应 |
|---|---|---|
| `POST /api/v2/auth/login` | `{ "username": "admin", "password": "..." }` | `AdministratorSession` + HttpOnly Cookie |
| `GET /api/v2/auth/session` | 无 body；Cookie | 新的 `AdministratorSession`，同时轮换 CSRF token |
| `POST /api/v2/auth/logout` | 无 body；Cookie + `X-CSRF-Token` | `204` 并过期 Cookie |

`AdministratorSession` 恰好包含 `authenticated: true`、UUID `user_id`、canonical `username`、固定
`role: "admin"` 和 `csrf_token` 五个字段。`admin` 是控制面 wire 常量，不在数据库持久化；当前 Schema
不存在 email、operator/viewer 或 role 列。登录 username 候选只允许 1–64 bytes 可打印 ASCII，经
trim ASCII whitespace 与 ASCII lowercase 后必须是 3–64 bytes、首尾字母数字且字符仅 `[a-z0-9._-]`；
`@` 明确非法。普通备份账户的 `accounts.username` 不属于管理角色系统。所有管理 mutation 位于
`/api/v2/admin/*`，都要求同源、
有效 Session 和与该 Session 绑定的 CSRF token。

这套管理合同只由 Server 与内置 React/Vite Web 消费。移动 `/v2` DTO、设备 Token/API Key、Agent
Schema、Android/iOS 和 FFI 仍使用原有数据面身份；协议修改时不得机械地把它们替换成管理 username。

失败响应不使用产品私有 `{error}`。服务端直接序列化 `sarmg-error=0.3.0` 的 Foundation ErrorEnvelope 顶层
`{code,message,retryable}`；`code` 只用小写字母开头的安全 ASCII 标识，当前没有 request ID 时不输出
占位字段，当前没有结构化细节时也不输出 `details`。429 的 envelope 标记 `retryable:true`，并保留
标准 `Retry-After` header；客户端应以 header 决定等待时间，不从 message 文本解析策略。

## DTO 版本字段

移动配置和入队 JSON 显式包含：

```json
{
  "product": "media-backup",
  "application_version": "0.2.0",
  "revision": 1,
  "state_epoch": "media-backup-mobile-v0.2-r1"
}
```

实际 DTO 还有服务地址、用户/资源、part size 等必填字段。未知字段和缺失字段均拒绝；示例不是可直接
发送的完整请求。

## 上传协议

1. create 声明资源 identity、总字节、BLAKE3、part 计划和 `storage_encoding=plain-v1`。
2. 服务端返回 upload ID 与缺失 part。
3. PUT 指定 index，只接受计划长度和 Hash。
4. complete 使用精确空对象/manifest，服务端复核全部 part、合并和总 Hash。
5. 返回 durable asset/resource identity；重复 complete 返回同一逻辑结果。

part size 必须在客户端和服务端上限内。最后一块可短，其余块长度由计划确定。不能通过 Content-Length
声明绕过实际 streaming byte count。

## 时间线与 cursor

`GET /v2/timeline` 支持 cursor、limit、trashed、favorite、archived、album/tag filters。cursor 是不透明
且绑定排序位置的服务端编码；客户端只保存/回传。列表可能在分页间变化，因此使用稳定 tie-breaker。

## 增量同步

`GET /v2/sync?after=sequence` 返回大于游标的有界变更。sequence 单调但不代表 wall clock。客户端先
事务应用一页，再持久化新 cursor；反向顺序会在崩溃时漏数据。若服务端明确要求 full sync，客户端
重新分页，不自行猜旧 cursor。

## 图库操作

PATCH 资产只接受当前可变字段。相册成员完整扫描与 batch upsert 的删除语义不同。Tag 创建、批量设置、
单项添加/移除都校验 owner。trash/restore 是软状态；DELETE 只接受 trashed 资产。

## API Key 与审计

API Key 明文只在创建响应出现一次，数据库存摘要。撤销立即使后续请求失败。审计使用 sequence 分页，
记录 actor/operation/resource/result，不记录 Secret 或媒体。

## 协议修改检查

任何 wire 变化同时修改 protocol crate、server handler、Android/iOS model、FFI/JNI、静态 contract gate、
数据库测试和文档。破坏性变化定义新当前 epoch，不保留双栈解析器。
