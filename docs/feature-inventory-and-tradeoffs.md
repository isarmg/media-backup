# Media Backup 完整功能与取舍清单

## 0. 开发者决策台账

本表覆盖 Server、Android、iOS、共享 Rust Agent、协议、存储和交付。分类只能取“核心、保障、可选、建议保留、开发运维”；复杂度按删除所需的跨端闭包评估。删除时必须同时清理 API、数据库、客户端状态、FFI、测试和文档。

| ID | 功能/特性与当前实现 | 实现/主要依赖 | 分类 | 复杂度 | 删除后的确定后果 | 最低验证 |
|---|---|---|---|---|---|---|
| MED-001 | 用户、设备和存储根管理 | Server admin API、SQLite、管理 Web | 核心 | 高 | 无法安全划分每个用户/设备的备份空间 | CRUD、权限、路径、quota |
| MED-002 | 管理员 Argon2 登录 | admin auth、users、登录预算 | 核心 | 中 | 管理 API/UI 无身份边界 | 正误密码、未知账号、限流 |
| MED-003 | Session、CSRF、Origin/Host 保护 | admin sessions、middleware、Cookie | 保障 | 高 | 登录态可被跨站利用或无法撤销 | TTL、撤销、unsafe method |
| MED-004 | API access token 摘要存储与撤销 | api_access、token scope、DB | 保障 | 高 | 移动端无法稳定认证；明文 token 落库会扩大泄露 | 创建、撤销、泄露面、并发 |
| MED-005 | 唯一当前移动协议 v0.2 revision 1 | protocol crate、Android/iOS/FFI | 核心 | 高 | 三端无法互操作；添加旧 alias 会扩大兼容负担 | contract 静态门、未知字段 |
| MED-006 | 上传预检和内容身份 | routes、agent-core、hash/size | 核心 | 高 | 客户端无法判断上传、跳过或冲突 | 已有/缺失/同内容/不同内容 |
| MED-007 | 分块/流式上传与有界正文 | upload route、tokio、storage | 核心 | 高 | 大媒体只能整文件缓冲或无法上传 | 截断、超长、慢流、取消 |
| MED-008 | 暂存文件到原子提交 | upload_commit、same-filesystem stage、fsync | 保障 | 高 | 崩溃可留下半文件或成功后数据不持久 | 各 rename/fsync 故障点 |
| MED-009 | 幂等请求/重复提交收口 | upload identity、DB 唯一约束 | 保障 | 高 | 移动网络重试会重复落盘或覆盖 | 同 ID 同内容/异内容 |
| MED-010 | 用户根内路径约束与安全解析 | storage、canonical relative paths | 保障 | 高 | 路径穿越或 symlink 可写出分配目录 | `..`、绝对、symlink、hardlink |
| MED-011 | 文件类型、扩展名和媒体元数据边界 | library、scanner contract | 保障 | 中 | 任意文件可污染媒体库或合法媒体被误拒 | MIME/扩展/大小/时间 |
| MED-012 | 内容哈希和数据库/磁盘一致性 | agent-core/database、doctor | 保障 | 高 | 去重、损坏检测和恢复判断失真 | 篡改、缺失、多余、重复 |
| MED-013 | quota 与磁盘空间保护 | users quota、storage checks | 保障 | 高 | 单用户可占满共享盘并影响全服务 | 并发上传、边界、释放 |
| MED-014 | library 列表与远端媒体查询 | Server API、Android/iOS RemoteLibrary | 核心 | 高 | 用户无法确认已备份内容或远端状态 | 分页、空态、权限、排序 |
| MED-015 | Android 媒体扫描与 album 映射 | MediaStore、DeviceAlbums | 核心 | 高 | Android 无法发现需备份的媒体 | 权限、删除、重复、album |
| MED-016 | Android WorkManager 后台调度 | BackupScheduler/Worker | 建议保留 | 高 | 只能前台手工备份，可靠性显著下降 | 约束、重试、进程重启 |
| MED-017 | Android SecureConfig | 平台安全存储、Kotlin client | 保障 | 中 | token/endpoint 可能明文泄露或无法持久化 | 重启、清除、错误数据 |
| MED-018 | Android Compose 状态与手工触发 | MainActivity、Worker status | 建议保留 | 中 | 后台仍可工作但用户无法配置/查看 | 权限、空态、错误态 |
| MED-019 | iOS PhotoKit 增量扫描 | PhotoScanner、change token | 核心 | 高 | iOS 无法枚举并增量备份照片/视频 | 权限变化、iCloud item、删除 |
| MED-020 | iOS 后台 URLSession 上传 | BackgroundUploader、delegate | 建议保留 | 高 | App 退到后台后大上传容易中断 | relaunch、task mapping、取消 |
| MED-021 | iOS Keychain 配置和 token | KeychainStore | 保障 | 中 | credential 失去平台级保护 | 存取、覆盖、无效记录 |
| MED-022 | SwiftUI 配置/进度/远端库 | ContentView、coordinator | 建议保留 | 中 | 核心引擎仍在但没有可用 iOS 界面 | 权限、进度、失败、重试 |
| MED-023 | 共享 Rust Agent Core | `crates/agent-core`、SQLite queue | 核心 | 高 | Android/iOS 会各自实现队列和状态，行为漂移 | 两端 contract 与恢复测试 |
| MED-024 | C FFI revision 与生成头文件 | mobile-ffi、JNI/Swift bridge | 核心 | 高 | 原生客户端无法调用共享核心 | ABI 名称、内存、错误、线程 |
| MED-025 | 客户端本地队列/检查点 | agent-core database、platform bridge | 保障 | 高 | 崩溃或断网后任务丢失/重复 | 重启、损坏、并发、终态 |
| MED-026 | Server 当前 Schema identity 和拒绝 migration ledger | metadata、schema fingerprint | 保障 | 高 | 错版/漂移库可能运行，旧兼容分支回归 | wrong version/SHA/ledger |
| MED-027 | Server 单实例/runtime lock | runtime_lock、database/data locks | 保障 | 高 | 双实例可并发提交同一文件和清理状态 | 双启动、维护锁 |
| MED-028 | `doctor` 只读 DB/存储/身份检查 | doctor、release checks | 开发运维 | 中 | 故障定位和上线验收变弱 | 健康/篡改/权限/空间 |
| MED-029 | 嵌入式管理 Web | `clients/web`、Rust include、routes | 建议保留 | 中 | 管理 API 保留但无内置 UI | HTML/CSS hash、auth、CSRF |
| MED-030 | `config/media-backup.env.example` 配置合同 | config parser、部署脚本 | 开发运维 | 低 | 字段来源不清晰，错误部署概率上升 | 字段与 parser 对照 |
| MED-031 | source-bound Server release 与 manifest | release.rs、build/test scripts | 开发运维 | 高 | 二进制、Web、头文件、服务文件可能混代 | missing/extra/tamper/relocate |
| MED-032 | Android/iOS/Server CI 与协议门禁 | workflows、contract script | 开发运维 | 高 | 任一端可在不兼容时独立发布 | Rust、Gradle、Xcode、FFI checks |
| MED-033 | 中文学习、流程、功能和运维文档 | README、`docs/` | 开发运维 | 低 | 跨端知识难以传递 | 链接、命令、字段抽查 |
| MED-034 | 明确不做旧协议/旧 Schema、云转码、社交分享和服务端识别 | 不存在兼容 route/reader | 核心 | 高 | 新增会改变隐私、容量、协议和迁移边界 | 独立设计与威胁评审 |

## 1. 功能清单

| 领域 | 当前功能 | 关键限制或取舍 |
|---|---|---|
| 平台 | Android 与 iOS 原生客户端 | 不提供桌面同步客户端 |
| 扫描 | MediaStore/PhotoKit、相册选择与排除 | 只能读取用户和系统授予的范围 |
| 后台 | WorkManager、后台 URLSession、持久队列 | 受移动系统调度限制，不承诺实时上传 |
| 上传 | 分块、缺块续传、幂等提交、配额 | 协议只接受 `plain-v1` |
| 完整性 | 分块与完整对象 BLAKE3 | Hash 证明一致，不提供机密性 |
| 存储 | 原始媒体、设备缩略图、账户内内容去重 | 服务端管理员可读取媒体 |
| 恢复 | 时间线分页、原图/视频单项与批量恢复 | 不实现跨设备相册协作 |
| 同步 | 单调 change sequence 和持久游标 | 不提供多主冲突合并 |
| 组织 | 相册、标签、收藏、归档 | 标签管理 UI 保持轻量 |
| 删除 | 回收站、撤销、受限永久删除 | 当前无自动清空保留策略 |
| 重复项 | 按原始 BLAKE3 与大小分组 | 不做视觉相似度识别 |
| 管理 | 管理员会话、普通用户、API Key、审计 | 浏览器与设备身份不可互换 |
| 监控 | health、doctor、Prometheus metrics | 不提供完整 OpenTelemetry 链路 |
| 发布 | 固定版本树、严格 manifest、移动端联合 CI | 同版本不可覆盖安装 |

## 2. 安全设计取舍

- 选择 TLS + 服务端原始字节，而不是端到端加密。优点是新设备无需旧密钥即可恢复，缺点是服务器
  管理员拥有媒体可见性。
- 选择设备生成缩略图，避免服务端解码管线和攻击面；代价是移动端初次处理更多。
- 选择四类独立凭据，减少权限混淆；代价是运维必须分别轮换和监控。
- 选择严格可信代理解析，不盲信转发头；代价是部署必须明确配置直接代理 CIDR。
- 选择当前 Schema 精确指纹，而不是运行时 migration；代价是升级必须停机并使用外部工具。

## 3. 与完整媒体平台的边界

本项目独立实现新设备恢复、相册同步、时间线、缩略图、增量同步、收藏/归档/标签、回收站、重复项、
API Key、审计和指标。明确不实现：人脸识别、语义搜索、地图、分享链接、视频转码、RAW 派生、合作
伙伴共享、多级作业集群和完整 Web 图库。这些能力会显著扩大依赖、算力、隐私和升级面，应作为独立
产品需求评估。

项目研究过 Immich `3.1.0` 的公开产品行为，但 Rust/Axum/SQLx 协议、SQLite Schema、Swift/Kotlin
客户端和资源均为独立实现。Immich 使用 AGPL-3.0；本项目第一方内容使用 Apache-2.0，禁止将参考副本
的代码、样式、资源、测试或 Schema 直接带入本仓库。

## 4. 明确不提供的兼容能力

- 不注册 `/v1`、未版本化 API、`/admin/api`、尾斜杠 fallback 或重定向 alias。
- 不读取或转换 0.1 数据库、移动凭据、队列、staging、FFI 或 JNI 名称。
- 不接受缺失 `product`、`application_version`、`revision`、`state_epoch` 或存储编码的宽松 JSON。
- 不在服务进程中提供 migrate、backup、restore 或 previous-key keyring。
- 不创建 `current`、`latest` 等可变发行链接，不覆盖已经存在的同版本目录。

这不是“暂未实现”的缺陷，而是降低长期兼容成本、让每个版本成为完整新契约的架构决定。

## 5. 后续功能判断标准

新增功能必须回答：是否属于可靠备份主目标、是否会让服务端必须解析媒体、是否扩大公开协议、是否
需要新的敏感数据、是否能在 Android/iOS 双端一致实现、是否拥有测试和运维恢复路径。若答案引入
跨版本兼容逻辑，应把转换放入 `sarmg-upgrade`，不能加入产品运行时。

## 6. 按用户旅程核对能力

### 6.1 首次安装与注册

| 步骤 | 系统行为 | 成功证据 | 常见失败及边界 |
|---|---|---|---|
| 安装 Server | 验证固定发行树并创建缺失的当前数据库 | readiness、release verify、doctor 全通过 | 不接管已有未知数据库，不在线迁移 |
| 创建管理员 | 首次初始化保存 Argon2 摘要 | 可建立受 CSRF 保护的 Session | 初始密码不是设备 Token |
| 创建用户/设备 bootstrap | 管理员明确授予当前账户 | 返回一次性/受限引导材料 | 不允许匿名自注册或跨账户绑定 |
| 移动端保存配置 | Keychain/Keystore 保存敏感凭据 | 重启后仍可读取且不进普通偏好 | 服务端不能恢复设备本地 Secret |
| 授予相册权限 | 系统选择器限定范围 | 扫描只看到授权资源 | “有限照片访问”不是应用错误 |

### 6.2 日常备份

扫描器读取原始资源元数据和字节，生成 BLAKE3 与缩略图，把任务写入移动 SQLite，再按当前 `/v2` 合同
创建会话、查询缺块、上传和提交。进度 UI 是本地任务投影；服务端最终资产/资源记录才是远端事实。

| 情况 | 当前语义 | 选择理由 |
|---|---|---|
| App 被系统终止 | 已持久任务下次调度继续 | 不依赖内存队列 |
| 网络在分块后中断 | 查询缺块并复用同一上传身份 | 避免重复传完整对象 |
| 相同字节再次出现 | 账户内按 Hash/大小复用内容 | 节约存储但不跨账户泄露存在性 |
| 本地照片删除 | 默认不删除 Server 副本 | “备份”优先于双向镜像 |
| 服务端配额不足 | 提交/准入明确失败并保留可诊断状态 | 不在磁盘满时静默丢数据 |

### 6.3 恢复与整理

时间线和同步序列提供稳定分页；恢复下载原始字节并由宿主写回系统照片库。相册、标签、收藏、归档、
回收站和重复项是服务端组织状态，不改变原始媒体 Hash。永久删除只接受回收站内对象，降低误删风险。

## 7. 服务端功能分解

| 子系统 | 输入 | 持久状态 | 输出/错误 | 容量边界 |
|---|---|---|---|---|
| 管理认证 | email/password、Cookie、CSRF | user、Session 摘要 | 管理页面/API envelope | body、来源、账户、Argon2 并发 |
| 设备认证 | bootstrap/Bearer Token | Token SHA-256、设备绑定 | 设备 API 授权 | Token 数、请求速率 |
| API Key | 管理员创建的 scope/Secret | Key 摘要、撤销状态 | 自动化授权 | scope、Key 数、准入 |
| 上传会话 | 资产 metadata、块计划 | session、chunk/commit 状态 | missing chunks、commit result | 正文、块数、并发、配额 |
| 文件存储 | 已校验 stage | 原始对象、缩略图 | 安全下载/恢复 | 根路径、空间、文件大小 |
| 同步 | account cursor | 单调 change sequence | 有界变更页 | page size、cursor 格式 |
| 图库组织 | asset/album/tag ID | 关系、收藏、归档、trash | 当前资源投影 | 数量、名称、权限 |
| 审计/指标 | 安全事件摘要 | audit rows/metrics | 管理查询、Prometheus | 不含 Secret/媒体正文 |

## 8. 移动客户端能力差异

| 领域 | Android | iOS/iPadOS | 共同合同 |
|---|---|---|---|
| 相册来源 | MediaStore 与系统权限 | PhotoKit 与 limited library | 只读取宿主授权集合 |
| 后台调度 | WorkManager | Background URLSession/系统任务 | 不承诺固定实时周期 |
| Secret | Android Keystore 包装存储 | Keychain | 不进入 Rust SQLite/日志 |
| 原生边界 | Kotlin/JNI | Swift/C ABI | `media-backup-mobile-v0.2-r1` |
| 恢复写入 | MediaStore | PhotoKit change request | 宿主负责权限与系统错误 |
| 构建证明 | Gradle + cargo-ndk | XcodeGen/Xcode + Rust targets | 两端协议 fixture 必须一致 |

移动端差异只存在平台宿主层；上传状态、Hash、JSON、错误和幂等语义由 Rust 核与合约统一。不能因为某
平台更容易实现就添加另一平台无法表达的隐式协议。

## 9. 数据与隐私分类

| 数据 | 存放位置 | 敏感性 | 备份要求 |
|---|---|---|---|
| 原始照片/视频 | Server data tree | 高，管理员可见 | 与 SQLite 同一组合状态 |
| 缩略图 | Server data tree，由设备生成 | 高，仍可识别内容 | 与原始对象关系一致 |
| 资产/相册/标签 metadata | Server SQLite | 高，包含时间和组织信息 | sidecar-aware 一致备份 |
| 设备 Token/API Key | 设备安全存储；Server 仅摘要 | Secret | 原值不可从 Server 恢复 |
| 管理 Session/CSRF | Cookie/内存与 Server 摘要 | Secret/短期 | 通常不作为业务恢复目标 |
| 移动队列 | App 私有 SQLite | 敏感运行状态 | 可重建但会影响未完成任务 |
| 审计 | Server SQLite | 敏感安全记录 | 随数据库保全并限制访问 |

## 10. 可靠性与失败语义

| 故障 | 不能声称的结论 | 当前恢复方式 |
|---|---|---|
| 客户端 HTTP timeout | “服务端一定没收到” | 以同一 ID 查询/重试，依赖幂等合同 |
| chunk 写完后崩溃 | “整个资产已提交” | 恢复 session，核对 missing chunks |
| 数据文件已写、DB 事务失败 | “资产可见” | 私有 stage/清理任务收口，不发布无 metadata 对象 |
| DB 记录完成、文件同步失败 | “持久成功” | 提交顺序与 doctor 暴露，不返回伪成功 |
| SQLite Schema 不符 | “可自动升级” | 只读拒绝，交给 `sarmg-upgrade` |
| Android/iOS 后台被终止 | “任务失败” | 等待系统下一次调度，从持久队列恢复 |

## 11. 需求取舍决策表

| 候选需求 | 当前决定 | 原因 | 若未来实现必须新增的证据 |
|---|---|---|---|
| 端到端加密 | 不提供 | 会改变缩略图、去重、恢复与 key 生命周期 | 双端密钥恢复、灾备和不可恢复风险设计 |
| 服务端视频转码 | 不提供 | 扩大解码攻击面、GPU/队列/衍生物状态 | sandbox、资源预算、持久作业与失败恢复 |
| 人脸/语义搜索 | 不提供 | 高隐私与模型供应链成本 | 明确同意、索引删除、算力与模型版本合同 |
| 分享链接 | 不提供 | 引入公开授权、撤销与滥用面 | 短期 Token、下载限流、审计和代理边界 |
| 自动清空回收站 | 暂不提供 | 错误策略会不可逆删除备份 | 保留配置、dry-run、审计、恢复窗口测试 |
| 跨账户去重 | 不提供 | 可形成内容存在性侧信道 | 隔离/加密/计费和泄漏 threat model |
| 桌面同步 | 不提供 | 双向文件冲突不等于移动媒体备份 | 独立产品状态机、平台矩阵与冲突语义 |

## 12. 功能完成定义

一项功能只有在以下各项同时满足时才算“完整”：Android/iOS 或明确适用平台实现；服务端严格验证；
持久状态与崩溃恢复已定义；身份和权限已最小化；容量/超时有界；正例与安全负例通过；doctor/指标能观测；
备份恢复资源合同明确；发行物真实构建；中文教程、流程、取舍和运维同步。只有 handler 或页面不算完成。
