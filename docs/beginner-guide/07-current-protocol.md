# 第 7 章：当前协议、上传与同步

## 唯一 namespace

业务 API 只位于 `/v2`。`/admin` 是静态管理入口，health/metrics 是运维入口。没有 `/v1`、未版本化、
旧项目名、尾斜杠 alias 或 fallback redirect。

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
