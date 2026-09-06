# Android 正式签名

应用 ID 为 `org.sarmg.mediabackup`，PKCS#12 alias 为 `media-backup-android-release`。
当前发布证书 SHA-256 为 `0cfc2811d48cdeab3e6d857029d879e001ab9531c06784b4d48d15a847771421`。
本次继续使用已有签名身份，不生成替代密钥、不发布 debug APK。

GitHub 仓库 `isarmg/media-backup` 的 `android-signing` 环境保存两个 Secrets：

- `MEDIA_BACKUP_ANDROID_SIGNING_PKCS12_BASE64`
- `MEDIA_BACKUP_ANDROID_SIGNING_PKCS12_PASSWORD`

环境部署规则仅放行已审定的 `v0.2.1` 标签。未来版本需先审查对应提交及发布工作流，再显式添加标签规则，不放行任意分支或 pull request。
私钥和密码不进入 Git、Release 附件或构建日志；发布任务只在临时目录生成权限受限的签名输入。

正式流水线执行 `assembleRelease`，并使用 `apksigner` 验证签名、唯一证书及以上指纹；同时检查 application ID 和唯一 `arm64-v8a` 原生库。
只有 Server、Android 和 iOS 全部完成后才创建 Release。iOS 制品明确标注 unsigned，不冒充具有 Apple provisioning 的可安装包。

签名私钥另有受限权限的离线备份。仓库管理员应将备份转存至自己的加密存储并妥善保存密码；GitHub Secrets 不能作为唯一可恢复备份。
