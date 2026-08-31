# 第 6 章：Android 与 iOS 客户端

## 共同状态机

```text
未配置 -> 保存当前 HTTPS/账号 -> 登录取得设备 Token
 -> 请求照片权限 -> 选择/排除相册 -> 扫描
 -> durable enqueue -> background upload -> sync cursor
 -> timeline browse -> authenticated download -> system restore
```

两端必须表达相同产品/version/revision/epoch 和 `plain-v1`，但不要求 UI 代码共用。

## Android 扫描

`MediaScanner` 通过 ContentResolver/MediaStore 分页读取媒体，使用稳定系统 ID、修改时间、MIME、大小和
相册关系构造候选。`DeviceAlbums` 管理选择/排除。不要在主线程读取大文件，也不要假设 content URI 是
普通文件路径。

`BackupScheduler` 配置 WorkManager 网络/电量策略，`BackupWorker` 驱动 Rust queue 与 `BackupApi`。
Worker 重启时从 durable state 继续，不能仅依赖 Compose 内存状态。

## Android Secret 与权限

服务地址、用户可见设置与 Token 分级保存；Token 由 Keystore 支持的加密存储保护。日志、Intent、
SavedState 和 crash report 不包含 Token。Android 版本化 package/app ID 只使用当前命名空间，不查找旧
preference、database 或 staging。

## iOS 扫描

`PhotoScanner` 通过 PhotoKit fetch result 和授权范围读取 asset/album，必要时请求 resource stream。
limited library 权限意味着可见集合会变化；扫描逻辑要把“不可见”与“已删除”区分，避免错误删除服务
端副本。

`BackupCoordinator` 管理扫描/队列/后台任务，`BackgroundUploader` 关联 URLSession task 与 durable job。
delegate 回调可在 App UI 不存在时发生，因此恢复 mapping 不能只放内存。

## iOS Secret 与恢复

Token 存 Keychain。下载完成先验证内容，再通过 PhotoKit performChanges 写入系统库；权限拒绝、空间
不足和 duplicate 写入分别呈现，不把服务端成功下载等同于系统恢复成功。

## FFI 规则

当前 C ABI 仅有 `mb_v0_2_r1_*`，header 仅有 `media_backup_v0_2_r1.h`。输入 UTF-8/JSON 的所有权和
输出 free 函数固定；panic 不穿越 FFI。Kotlin JNI 类和 Swift `_silgen_name` 必须由静态门禁逐项匹配。

## UI 可观察状态

至少区分扫描中、待上传、正在上传、可重试失败、永久拒绝、同步中和恢复结果。显示统计不能成为状态
事实源；重启后从 SQLite/API 重建。取消只是停止本轮工作，是否删除 durable job 必须由明确用户动作决定。
