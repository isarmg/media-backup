# 第 5 章：存储、状态机与可靠性

## Rooted filesystem

所有媒体 key 是数据库生成/验证的逻辑标识，不是客户端任意路径。服务端从已验证 `DATA_DIR` 目录句柄
逐组件打开，拒绝 `..`、绝对路径、空组件、symlink、特殊文件和 hardlink alias。检查和使用尽量绑定同
一实体，降低 TOCTOU。

## 上传状态

典型状态包括 created、receiving、ready/committing、completed、failed。part record 保存 index、长度和
Hash；重传相同 part 幂等，不同内容冲突。complete 只能由一个 owner 推进，崩溃点通过数据库 record 与
staging/final 文件 identity 在重启后收敛。

## 原子发布

文件先在目标文件系统的私有临时位置完整写入、fsync 并验证，再用 no-clobber/原子 rename 发布。数据库
提交和 rename 无法组成一个底层原子操作，因此代码按可恢复阶段排序，保证任何中断点都能识别“尚未
发布、已发布未记账、已记账”并安全重试。

## Dedupe 与删除

BLAKE3/size 用于寻找完全相同内容。逻辑资源按账户授权，物理对象可被引用计数复用。删除资产不等于
立即删物理 blob；必须在事务中证明无引用，再在受控文件操作中清理。Hash 不是访问控制。

## 当前 Schema 验证

启动不会直接用写连接“看看能不能迁移”。它复制 main/WAL/journal 到私有临时 generation，读取唯一
metadata，重新计算排除 metadata/sqlite internal 的 DDL fingerprint，并执行结构不变量。原件在拒绝
路径不应产生新的 sidecar 或字节变化。

## 两把资源锁

服务按固定顺序锁定数据库 identity 和 DATA_DIR identity，防止两个实例共享任一资源。锁文件的父链和
实体也验证。`doctor`/维护工具遵循同一顺序，否则可能死锁或把不同资源拼成一个假实例。

## 容量与反压

配额控制账户最终媒体；上传 staging、SQLite WAL、移动 spool 和缩略图也需要单独容量监控。磁盘满时
不得把 partial file 当完整对象，客户端保留可重试任务。所有队列、body、分页和并发都有代码上限。

## 恢复判断

先以 durable metadata 和内容 Hash 判断，不按临时文件名或 mtime 猜测。无法证明所有权时留下证据并
失败关闭；清理任务只能删除符合当前命名、权限、年龄和数据库无引用等全部条件的对象。
