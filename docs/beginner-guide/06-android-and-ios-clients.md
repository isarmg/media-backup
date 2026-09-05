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
Worker 重启时从 durable state 继续，不能仅依赖 Compose 内存状态。达到本轮 40 个新资源上限时不会全量
替换远端相册成员；但 Android 14 selected-media grant 当前没有被标成“不完整扫描”，维护者必须知道它
仍可能把不可见成员从相册关系中移除。

“从 durable state 继续”必须按状态理解：`ready` job 会复用已经持久化的 prepared JSON 和 part；当前
`retry_wait` 到期后却会重新调用 `prepare_file`。Android/iOS 扫描器通常把系统媒体导出为 staging 临时源，
并要求准备成功后删除它；若随后上传失败，下一轮可能因源文件已不存在而无法复用仍在磁盘上的 part。
准备失败或 `preparing` 崩溃残留也没有自动按 job 目录清理。维护者不能把“SQLite 有任务”直接等价为
“所有上传失败都可恢复”，也不能用清空整个 staging 的方式排障。

## Android Secret 与权限

服务地址、用户可见设置与 Token 分级保存；Token 由 Keystore 支持的加密存储保护。日志、Intent、
SavedState 和 crash report 不包含 Token。Android 正式 package/app ID 只使用当前命名空间，不查找旧
preference、database 或 staging。

正式 application ID 是 `org.sarmg.mediabackup`，Kotlin 路径、namespace、JNI 导出名和 APK badging 必须
同时一致。Debug APK 只用于 CI/开发，永远不取得正式 Secret；Release 使用受保护 Environment 中两个
PKCS#12 Secret，并在发布前核对唯一 signer、固定证书指纹和唯一 `arm64-v8a` JNI。因为本项目不兼容旧
代码，新 identity 不提供旧 package/证书的升级安装路径，也不保留旧 JNI 符号。

## iOS 扫描

`PhotoScanner` 通过 PhotoKit fetch result 和授权范围读取 asset/album，必要时请求 resource stream。
limited library 权限意味着可见集合会变化。当前扫描结果没有携带“非完整可见集”标志，而相册同步总是
`replace_members=true`；因此隐藏于 limited grant 外的成员可能从远端相册关系中移除（媒体 asset 本身不
会因此删除）。这是当前边界，不是已经解决的保障。

`BackupCoordinator` 管理扫描/队列/后台任务，`BackgroundUploader` 用 `taskDescription` 关联 URLSession
task 与 durable job。当前 `AppDelegate` 只注册 BGProcessingTask，没有实现 background URLSession 的
relaunch completion handoff；因此只能保证现有 delegate 生命周期内的回调，不能宣称杀进程后已闭环。
另外 `runBackup` 在 part 仍由系统异步传输时就同步相册，新资产成员可能要到后续运行才能补齐。

## iOS Secret 与恢复

Token 存 Keychain。当前恢复路径检查 HTTP 成功并把临时下载文件交给 PhotoKit `performChanges`，但尚未
调用 Rust `restore_file` 按 manifest size/BLAKE3 复核；因此下载成功不能表述为内容已端到端验证。权限
拒绝、空间不足和照片库写入失败也都不能等同于服务端恢复成功。Android 的 MediaStore 恢复同样没有
连接该 Rust 校验 helper。

## FFI 规则

当前 C ABI 为 Foundation revision 2：`mb_*_v2` 接收显式指针/长度和 `SarmgFfiResultV2` 输出，返回状态码。
生成 Header 只有 `media_backup_ffi_v2.h`；结果中的字节和错误消息均有明确长度，必须通过
`sarmg_ffi_result_free_v2` 释放整个结果，不能复制后重复释放。旧 NUL 字符串入口与线程局部错误已删除。
Swift 通过 XCFramework 的 `MediaBackupRust` C module 导入 Header，不再使用手写 `_silgen_name`；
Kotlin 使用 `NativeBridgeV2`，JNI 失败抛出 Foundation 映射的异常，不把默认值当成功。两个宿主都先验证
ABI revision；业务配置仍严格验证当前 product/version/revision/state_epoch，ABI revision 不等于状态 epoch。
Rust panic 经共享边界转成 255，panic 内容不写入宿主日志；`panic=abort` 构建会被拒绝。
移动 `part_size` 限制为 1 字节至 64 MiB，单文件最多 4096 个分块；超限配置在创建数据库前拒绝，
已知超限文件在创建分块目录前拒绝。分块缓冲区使用可失败的分配，避免配置触发无界分配。
本机 C 动态库验收和 Rust 测试已通过，Android/iOS 原生运行验收必须另行执行，不能用静态门禁代替。

## UI 可观察状态

至少区分扫描中、待上传、正在上传、可重试失败、永久拒绝、同步中和恢复结果。显示统计不能成为状态
事实源；重启后从 SQLite/API 重建。取消只是停止本轮工作，是否删除 durable job 必须由明确用户动作决定。
