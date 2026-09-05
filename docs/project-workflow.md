# Media Backup 工作流程与流程树

## 1. 总流程树

```text
Media Backup
├─ 开发
│  ├─ 修改协议/服务端/移动端
│  ├─ 运行契约、格式、Clippy、测试
│  └─ CI 分别验证 Rust、Android、iOS 与发行归档
├─ 运行
│  ├─ 管理员登录并创建普通用户
│  ├─ 移动端登录、选择相册、扫描媒体
│  ├─ 本地队列分块上传并幂等提交
│  ├─ 服务端维护时间线、组织和变更序列
│  └─ 新设备分页下载并写回系统照片库
└─ 运维
   ├─ 验证不可变发行归档
   ├─ 安装固定物理版本并配置 TLS
   ├─ health/metrics/doctor 监测
   └─ 独立升级工具执行一致性备份、恢复或版本转换
```

## 2. 服务启动流程

正式启动按以下顺序失败关闭：

1. 启动脚本、systemd 和进程分别确认运行主机为 Linux x86_64。
2. 确认命令是 `serve-release`，进程位于 manifest 声明的固定物理版本目录。
3. 校验源码 revision、`x86_64-unknown-linux-gnu` target、API、Schema、FFI、Web 与全树文件摘要和权限。
4. 解析环境配置，使用 Foundation 规范化并验证 `BOOTSTRAP_ADMIN_USERNAME`，再验证生产 HTTPS、可信代理和路径组合。
5. 按稳定顺序取得数据库和 `DATA_DIR` 相邻的运行锁。
6. 数据库不存在时排他创建当前 Schema；存在时在私有副本上验证元数据与现场指纹。
7. 启动时先协调上传提交、无引用 blob 回收和 orphan commit staging，再启动每 120 秒一次、跳过错过 tick
   的周期协调任务；停机诊断可用 `reconcile scan` 走相同持锁路径。
8. Foundation Runtime 统一监听、信号处理、健康/诊断和有界优雅关闭；上传协调作为 Degrading 任务注册。
   中断时的 SQLite/文件系统持久状态仍由下一次启动的产品协调逻辑处理。

## 3. 管理员与设备接入

```text
首次启动环境中的 BOOTSTRAP_ADMIN_USERNAME + BOOTSTRAP_ADMIN_PASSWORD
  -> sarmg-admin-auth 对 username 执行 trim ASCII whitespace + ASCII lowercase
  -> 要求 canonical 3..64 bytes、首尾字母数字、字符仅 [a-z0-9._-]，明确拒绝 @
  -> Foundation Admin Core 在没有管理员时创建 _sarmg_administrators 记录
  -> POST /api/v2/auth/login {username,password}
  -> 精确 AdministratorSession + HttpOnly Cookie
  -> GET /api/v2/auth/session 轮换 CSRF
  -> /api/v2/admin/* 创建普通备份用户
  -> 手机 /v2/auth/bootstrap
  -> 设备 Token 安全存储
```

浏览器、设备、API Key、指标 Token 是四条独立授权链。浏览器登录由 Foundation 根据真实 socket peer
和规范化账户实施准入，并以并发上限和超时保护 Argon2；代理来源解析只属于产品移动业务授权链。
这里的 `_sarmg_administrators.username` 只属于 Server 管理面；移动端登录继续使用 `accounts.username`，设备/API Key、
移动队列 Schema、Android/iOS 请求和 FFI 均未改成管理身份，也不会因同名而获得 Administrator 权限。

## 4. 扫描与入队

Android 通过 MediaStore、iOS 通过 PhotoKit 读取用户授权范围。扫描结果经过相册选择/排除规则；Android
达到本轮新资源上限时会关闭 `replace_members`，但 Android selected-media 与 iOS limited 权限当前都没有
形成可传给 Server 的“可见集不完整”证明，仍可能移除不可见的相册成员关系。原始媒体和缩略图形成资源
描述后，任务先写入 `agent-v0.2-r1.sqlite`，staging 使用 `backup-staging-v0.2-r1`，然后才交给系统后台调度。

这里的“持久队列”不是无条件恢复保证。当前实现只会直接复用 `ready` 的 `prepared_json`；上传失败进入
`retry_wait` 后，即使该 JSON 和分块仍在，到期取队列仍会重新读取源文件。若宿主在准备成功后按
`remove_source_after_prepare` 删除了导出临时源，网络上传首次失败便可能在后续重试中持续报源文件不存在。
此外，准备中途失败或进程在 `preparing` 且尚未保存 JSON 时留下的 job staging 目录不会被自动精确清理。
排障时只能定位并保全单个 job 证据，不能删除整个 `prepared/`；这两项是当前未实现的可靠性保障。

## 5. 分块上传与提交

```text
创建 upload
  -> 服务端返回 upload_id/缺失分块
  -> 客户端逐块 PUT
  -> 服务端限制大小并验证分块
  -> complete 请求
       ├─ 分块不完整：返回可重试状态
       ├─ Hash/协议不符：拒绝
       └─ 完整：合并临时对象、验证 BLAKE3、原子提交数据库与最终文件
```

同一资源重复提交必须返回同一逻辑结果；账户内相同内容可复用对象，但资源归属与权限仍按账户隔离。
临时文件和最终路径都经过 rooted filesystem 校验，禁止路径逃逸、符号链接和特殊文件。

## 6. 图库与增量同步

所有会影响移动视图的变更取得单调 sequence。客户端保存上次游标并请求 `/v2/sync?after=...`；完整
时间线使用不透明分页 cursor，不能解析或自行构造。收藏、归档、标签、相册关系、回收站和重复组都
写入同一当前 Schema。永久删除先验证资产处于回收站，在一个事务中删除 asset/resource 并写 change/audit；
最后一个引用消失后，blob 行继续作为 durable 物理回收意图。协调器在 SQLite 删除事务仍打开时执行
rooted unlink：成功后提交删行，失败则回滚并保留行。请求即时收口返回 204，仍待重试返回 202；启动、
每 120 秒周期和手工 `reconcile scan` 都会继续处理，不会因用户可见删除已经提交而返回可诱导盲重放的 500。

## 7. 恢复流程

新设备登录后分页取得时间线，先下载鉴权缩略图供选择，再按单项或批量请求原始资源，最后交给
MediaStore/PhotoKit 写入系统照片库。当前 Android/iOS 宿主会检查 HTTP 成功和 `plain-v1` 合同，但恢复
路径尚未调用 Rust `restore_file`，因此不能宣称在写入照片库前再次按 size/BLAKE3 做端到端校验；这是
应保留在测试和后续计划中的明确边界。服务端保存原始字节，所以不存在服务端解密步骤或设备密钥依赖。

## 8. 发行流程

```text
干净 checkout + 精确 v0.2.0 tag
  -> 注入完整 Git revision 的 release build
  -> 构造唯一发行目录
  -> 写入严格 manifest
  -> 真实二进制 verify-release
  -> 打包 + SHA256SUMS
  -> 解包部署测试与篡改负例
  -> Android 从受保护 Environment 取得全新 PKCS#12，assembleRelease
  -> apksigner 唯一 signer/固定指纹 + aapt2 application ID + arm64-v8a ABI 复验
  -> GitHub Release（禁止覆盖既有资产）
```

Android 与 iOS 制品使用同一版本和移动 epoch；静态门禁发现的身份/ABI 漂移会阻止发行，运行行为仍需
各平台测试覆盖，不能把静态字符串门禁当成完整端到端证明。普通 CI 只构建 Debug/unsigned 移动制品，
不得取得 Android 正式 Secret；当前签名身份不接受旧 package、旧证书或旧 Secret 名作为 fallback。

## 9. 备份、恢复和升级流程

产品仓库没有这些写入命令。运维方停止服务并确认运行锁释放，再由 `sarmg-upgrade` 同时处理 SQLite
完整 generation 与 `DATA_DIR`。操作结束后先离线验证，再启动当前版本并运行 `doctor`。不能只复制
SQLite 主文件，也不能让产品自动猜测非当前状态。

## 10. Foundation 与管理 Web 构建流程

```text
Node 26.7.0 + clients/web/package-lock.json
  -> npm ci 安装精确依赖
  -> build 自动先运行 check:foundation
       ├─ assertAdministratorWebToolchain 检查 Node/React/Vite/TypeScript/types
       ├─ 检查 @sarmg/* 依赖来源与 lockfile
       ├─ 检查 tokens/reset/accessibility 的 SHA-256
       └─ 检查 React 根节点、data-sarmg-scope 与唯一当前入口
  -> TypeScript strict typecheck
  -> Vite 7 输出 dist/index.html + assets/admin.js + assets/admin.css
  -> Rust Server 把这三个产物编译进二进制
  -> release manifest 再绑定相同三个字节
```

四个 `@sarmg/*` 包始终从 Foundation GitHub Release `v0.3.0` 的对应 `.tgz` URL 安装；lockfile 绑定
实际归档的 `sha512` integrity，使独立 checkout/CI 不依赖同级目录。Rust crate 同时锁定版本 `=0.3.0`
和完整 Git revision `1fe326081cfd896f05ff502e80f99504797c14c6`。开发命令只有：

```bash
cd clients/web
npm ci
npm run check:foundation
npm run build
```

`@sarmg/contracts` 拥有管理员 wire 类型，`@sarmg/http-client` 拥有严格 ErrorEnvelope/同源请求，
`@sarmg/admin-web` 拥有认证状态机、React Hook 和 Vite/TS/toolchain 基线，`@sarmg/design-tokens` 拥有
scoped reset/accessibility/tokens。产品只在 `src/api.ts` 保留备份业务响应 guard，在 `src/styles.css` 保留
颜色、字体、卡片、布局和产品组件。生产浏览器不访问 Foundation 仓库、npm registry 或 CDN，也没有
vendored CSS、并行入口或运行时 fallback。
