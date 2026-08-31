# 第 5 章：存储、状态机与可靠性

## Rooted filesystem

所有媒体 key 是数据库生成/验证的逻辑标识，不是客户端任意路径。服务端从已验证 `DATA_DIR` 目录句柄
逐组件打开，拒绝 `..`、绝对路径、空组件、symlink、特殊文件和根目录逃逸。发布时以 `linkat` 创建
no-replace 硬链接并核对 source/destination 的 device+inode，再 fsync 目标父目录；因此硬链接本身是当前
发布机制的一部分，不能笼统写成“拒绝所有 hardlink”。`DATA_DIR` 必须由服务账户独占写权限。

## 上传状态

Server `uploads.state` 在当前主链使用 `uploading`/`complete`，细分提交阶段由 `commit_state` 表达：
`receiving -> commit_started -> finalizing -> committed`，另有 `unknown`/`failed`。part record 保存 index、
长度和 Hash；重传相同 part 幂等，不同内容冲突。同一进程内每个 upload 由 keyed mutex 串行 complete，
跨进程由数据库状态/唯一约束和运行锁收敛，并不存在持久化的通用 owner/lease 字段。

## 原子发布

文件先在目标文件系统的私有 stage 完整写入、fsync 并验证，再用 no-clobber `linkat` 把同一 inode 发布
到最终 key、核对 identity/Hash 并 fsync 父目录，最后清理 stage。数据库提交和文件发布无法组成一个
底层原子操作，因此代码按可恢复阶段排序，使协调器能识别“未发布、已发布未记账、已记账”等状态。

## Dedupe 与删除

BLAKE3/size 用于寻找完全相同内容。逻辑资源按账户授权，物理对象可被引用计数复用。永久删除先提交
asset/resource 与审计的用户可见事务；此后没有 resource 引用的 blob 行就是 durable 回收意图。协调器
用 `DELETE ... RETURNING` 取得写锁，在同一 SQLite 事务内执行 rooted unlink：unlink 失败则回滚并保留
行与文件，成功才提交删行。请求能立即收口时返回 204；仍需协调时返回 202，并由启动、120 秒周期或
`reconcile scan` 重试。Hash 不是访问控制。

## 当前 Schema 验证

启动不会直接用写连接“看看能不能迁移”。它复制 main/WAL/journal 到私有临时 generation，读取唯一
metadata，重新计算排除 metadata/sqlite internal 的 DDL fingerprint，并执行结构不变量。原件在拒绝
路径不应产生新的 sidecar 或字节变化。

## 两把资源锁

服务按固定顺序锁定数据库 identity 和 DATA_DIR identity，防止两个实例共享任一资源。锁文件的父链和
实体也验证。`doctor`/维护工具遵循同一顺序，否则可能死锁或把不同资源拼成一个假实例。

## 容量与反压

配额控制账户最终媒体；上传 staging、SQLite WAL、移动 spool 和缩略图也需要单独容量监控。磁盘满时
不得把 partial file 当完整对象，客户端保留可重试任务。HTTP body、分页、上传 admission 和每轮移动
扫描/上传有明确上限；SQLite job 总量没有统一硬上限，仍需容量监控和产品级保留策略。

## 恢复判断

先以 durable metadata 和内容 Hash 判断，不按临时文件名或 mtime 猜测。无法证明所有权时留下证据并
失败关闭；清理任务只能删除符合当前命名、权限、年龄和数据库无引用等全部条件的对象。
