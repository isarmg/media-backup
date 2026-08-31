# 第 10 章：源码路线、练习与术语表

## 四周阅读路线

### 第一周：契约与入口

读 workspace manifest、protocol、server main/routes/config。画出四种身份和每类路由。练习为一个无状态
GET 写 router test，观察 request ID 与错误 envelope。

### 第二周：上传与存储

读 upload/commit/storage/rooted_fs/database tests。为“同 index 不同 bytes”写负例，逐个标出 fsync、
rename、transaction 和 crash point。

### 第三周：移动端

读 agent-core Schema/job、mobile-ffi、Android Worker、iOS Coordinator。画出系统终止后恢复路径，运行
epoch gate，并证明旧命名空间不会被读取。

### 第四周：发布与运维

读 release.rs、manifest writer、build/test deployment、systemd。构建临时归档，分别篡改文件、权限、
添加 extra file，确认验证拒绝。

## 推荐练习

1. 构造在 WAL 中提交 current Schema 的 fixture，证明验证不改原件。
2. 模拟上传 complete 在 rename 前后中断，说明重启收敛结果。
3. 用不可信 forwarded header 请求，验证 client identity 不被伪造。
4. 让移动 sync 在事务提交前终止，证明 cursor 不会提前推进。
5. 对 release tree 增加 symlink/hardlink/extra file，观察失败信息。

## 术语表

| 术语 | 含义 |
|---|---|
| Asset | 用户可见的一张照片或视频逻辑实体 |
| Resource | 原始媒体或缩略图等具体内容对象 |
| Blob | DATA_DIR 中按内容 identity 保存的字节 |
| BLAKE3 | 内容完整性/完全重复判断 Hash |
| Part | resumable upload 的编号分块 |
| Cursor | 服务端生成、客户端原样保存的不透明分页位置 |
| Sequence | 图库变更的单调序号 |
| FFI/JNI | Rust 与 Swift/Kotlin 的二进制调用边界 |
| Epoch | 一组不兼容状态/协议的当前代 |
| WAL | SQLite 预写日志 sidecar |
| CSRF | Cookie 浏览器写请求的跨站伪造防护 |
| Argon2 | 密码 Hash 算法 |
| Rooted FS | 所有路径操作锚定已验证根目录的文件系统策略 |
| Idempotent | 相同请求重试得到同一效果而非重复副作用 |
| Fail closed | 无法证明安全或身份时拒绝 |
| Source-bound | binary 身份绑定精确源码 revision |

## 继续查阅

- [工作流程与流程树](../project-workflow.md)
- [功能与取舍清单](../feature-inventory-and-tradeoffs.md)
- [运维文档](../operations.md)
- 当前行为最终回到源码、Schema、manifest 和测试核对。
