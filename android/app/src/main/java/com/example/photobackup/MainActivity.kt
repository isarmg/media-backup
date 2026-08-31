package com.example.photobackup.v02

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.Typography
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.ContextCompat
import androidx.work.WorkInfo
import androidx.work.WorkManager
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.text.DateFormat
import java.util.Date

private val Ink = Color(0xFF17332B)
private val Pine = Color(0xFF176B57)
private val Mint = Color(0xFFBFE2D2)
private val Cream = Color(0xFFF8F3E9)
private val Paper = Color(0xFFFFFCF5)
private val Coral = Color(0xFFE9684A)
private val Muted = Color(0xFF61756D)
private val Border = Color(0xFFD8E0D9)

private enum class PermissionAction { SAVE, BACKUP }

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        BackupScheduler.syncAutomatic(this, SecureConfig(this))
        setContent { BackupTheme { BackupScreen(this) } }
    }
}

@Composable
private fun BackupTheme(content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = lightColorScheme(
            primary = Pine,
            onPrimary = Color.White,
            secondary = Coral,
            background = Cream,
            surface = Paper,
            onSurface = Ink,
            outline = Border,
        ),
        typography = Typography(
            displaySmall = TextStyle(
                fontFamily = FontFamily.Serif,
                fontWeight = FontWeight.Black,
                fontSize = 38.sp,
                lineHeight = 42.sp,
            ),
            headlineMedium = TextStyle(
                fontFamily = FontFamily.Serif,
                fontWeight = FontWeight.Bold,
                fontSize = 25.sp,
            ),
            titleLarge = TextStyle(fontWeight = FontWeight.Bold, fontSize = 19.sp),
            bodyLarge = TextStyle(fontSize = 16.sp, lineHeight = 23.sp),
            bodyMedium = TextStyle(fontSize = 14.sp, lineHeight = 20.sp),
            labelLarge = TextStyle(fontWeight = FontWeight.Bold, fontSize = 14.sp),
        ),
        content = content,
    )
}

@Composable
private fun BackupScreen(context: Context) {
    val config = remember { SecureConfig(context) }
    val workManager = remember { WorkManager.getInstance(context) }
    val scope = rememberCoroutineScope()
    var serverUrl by remember { mutableStateOf(config.serverUrl) }
    var username by remember { mutableStateOf(config.username) }
    var password by remember { mutableStateOf(config.password) }
    var autoBackup by remember { mutableStateOf(config.autoBackup) }
    var wifiOnly by remember { mutableStateOf(config.wifiOnly) }
    var chargingOnly by remember { mutableStateOf(config.chargingOnly) }
    var backupPhotos by remember { mutableStateOf(config.backupPhotos) }
    var backupVideos by remember { mutableStateOf(config.backupVideos) }
    var cameraOnly by remember { mutableStateOf(config.cameraOnly) }
    var deviceAlbums by remember { mutableStateOf<List<DeviceAlbum>>(emptyList()) }
    var selectedAlbumIds by remember { mutableStateOf(config.selectedAlbumIds) }
    var snapshot by remember { mutableStateOf(config.snapshot()) }
    var workInfo by remember { mutableStateOf<WorkInfo?>(null) }
    var notice by remember { mutableStateOf("") }
    var connectionState by remember { mutableStateOf("尚未检测") }
    var checkingConnection by remember { mutableStateOf(false) }
    var remoteAssets by remember { mutableStateOf<List<RemoteAsset>>(emptyList()) }
    var libraryLoading by remember { mutableStateOf(false) }
    var showingTrash by remember { mutableStateOf(false) }
    var duplicateGroupCount by remember { mutableStateOf(0) }
    var tagName by remember { mutableStateOf("") }
    var remoteThumbnails by remember { mutableStateOf<Map<String, Bitmap>>(emptyMap()) }
    var pendingAction by remember { mutableStateOf<PermissionAction?>(null) }

    LaunchedEffect(Unit) {
        deviceAlbums = withContext(Dispatchers.IO) { runCatching { DeviceAlbums.list(context) }.getOrDefault(emptyList()) }
        if (!config.albumSelectionConfigured) {
            selectedAlbumIds = deviceAlbums.mapTo(mutableSetOf()) { it.id }
            config.selectedAlbumIds = selectedAlbumIds
        }
        while (isActive) {
            snapshot = config.snapshot()
            workInfo = withContext(Dispatchers.IO) {
                runCatching {
                    val infos = workManager.getWorkInfosByTag(BackupScheduler.TAG).get()
                    infos.firstOrNull { it.state == WorkInfo.State.RUNNING }
                        ?: infos.firstOrNull { it.state == WorkInfo.State.ENQUEUED || it.state == WorkInfo.State.BLOCKED }
                        ?: infos.lastOrNull()
                }.getOrNull()
            }
            delay(1_200L)
        }
    }

    val permissionLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestMultiplePermissions(),
    ) {
        val action = pendingAction
        if (hasMediaAccess(context, backupPhotos, backupVideos)) {
            BackupScheduler.syncAutomatic(context, config)
            if (action == PermissionAction.BACKUP) {
                BackupScheduler.enqueueNow(context, config)
                val waiting = config.snapshot().copy(
                    state = "waiting",
                    message = if (wifiOnly) "任务已提交，等待 Wi-Fi 和系统调度" else "任务已提交，等待系统调度",
                    lastRunAt = System.currentTimeMillis(),
                )
                config.saveSnapshot(waiting)
                snapshot = waiting
            }
            notice = if (action == PermissionAction.BACKUP) "备份任务已提交" else "自动备份策略已启用"
        } else {
            if (action == PermissionAction.SAVE && autoBackup) {
                autoBackup = false
                config.autoBackup = false
                BackupScheduler.syncAutomatic(context, config)
            }
            notice = "未获得所选媒体权限，无法执行该备份策略"
        }
        pendingAction = null
    }

    fun persistSettings(): Boolean {
        val normalizedUrl = serverUrl.trim().trimEnd('/')
        if (!normalizedUrl.startsWith("https://")) {
            notice = "服务器地址必须使用 https://，媒体传输不允许明文 HTTP"
            return false
        }
        if (username.isBlank() || password.isBlank()) {
            notice = "请输入登录账号和密码"
            return false
        }
        if (!backupPhotos && !backupVideos) {
            notice = "照片和视频至少选择一项"
            return false
        }
        config.serverUrl = normalizedUrl
        config.username = username
        config.password = password
        config.autoBackup = autoBackup
        config.wifiOnly = wifiOnly
        config.chargingOnly = chargingOnly
        config.backupPhotos = backupPhotos
        config.backupVideos = backupVideos
        config.cameraOnly = cameraOnly
        config.selectedAlbumIds = selectedAlbumIds
        return true
    }

    fun requestAction(action: PermissionAction) {
        if (!persistSettings()) return
        if (action == PermissionAction.SAVE && !autoBackup) {
            BackupScheduler.syncAutomatic(context, config)
            notice = "设置已保存，自动备份已关闭"
            return
        }
        pendingAction = action
        if (hasMediaAccess(context, backupPhotos, backupVideos)) {
            BackupScheduler.syncAutomatic(context, config)
            if (action == PermissionAction.BACKUP) {
                BackupScheduler.enqueueNow(context, config)
                val waiting = config.snapshot().copy(
                    state = "waiting",
                    message = if (wifiOnly) "任务已提交，等待 Wi-Fi 和系统调度" else "任务已提交，等待系统调度",
                    lastRunAt = System.currentTimeMillis(),
                )
                config.saveSnapshot(waiting)
                snapshot = waiting
                notice = "备份任务已提交"
            } else {
                notice = "设置已保存，自动备份已启用"
            }
            pendingAction = null
        } else {
            permissionLauncher.launch(mediaPermissions(backupPhotos, backupVideos))
            notice = "请授权所选照片和视频范围"
        }
    }

    fun refreshLibrary(trashed: Boolean = showingTrash) {
        if (!persistSettings()) return
        libraryLoading = true
        scope.launch {
            runCatching {
                withContext(Dispatchers.IO) {
                    val api = BackupApi(config.serverUrl, config.bearerToken)
                    if (config.bearerToken.isBlank()) {
                        config.bearerToken = api.bootstrap(config.username, config.password, Build.MODEL)
                    }
                    val assets = RemoteLibrary.timeline(api, trashed)
                    config.librarySyncSequence = RemoteLibrary.advanceSync(api, config.librarySyncSequence)
                    val thumbnails = buildMap {
                        assets.take(12).forEach { asset ->
                            runCatching { RemoteLibrary.thumbnail(api, asset) }.getOrNull()?.let { bytes ->
                                BitmapFactory.decodeByteArray(bytes, 0, bytes.size)?.let { put(asset.id, it) }
                            }
                        }
                    }
                    Triple(assets, api.duplicateGroups().length(), thumbnails)
                }
            }.onSuccess {
                remoteAssets = it.first
                duplicateGroupCount = it.second
                remoteThumbnails = it.third
                notice = if (trashed) "回收站已刷新" else "服务器时间线已刷新"
            }.onFailure { notice = "加载图库失败：${it.message}" }
            libraryLoading = false
        }
    }

    val active = workInfo?.state == WorkInfo.State.RUNNING ||
        workInfo?.state == WorkInfo.State.ENQUEUED || workInfo?.state == WorkInfo.State.BLOCKED
    val progress = workInfo?.progress
    val completed = progress?.getInt(BackupWorker.PROGRESS_COMPLETED, snapshot.completed) ?: snapshot.completed
    val total = progress?.getInt(BackupWorker.PROGRESS_TOTAL, snapshot.discovered) ?: snapshot.discovered
    val activeMessage = progress?.getString(BackupWorker.PROGRESS_MESSAGE)?.takeIf { it.isNotBlank() }
        ?: snapshot.message
    val stage = progress?.getString(BackupWorker.PROGRESS_STAGE) ?: when {
        workInfo?.state == WorkInfo.State.ENQUEUED -> "等待"
        snapshot.state == "success" -> "完成"
        snapshot.state == "error" -> "注意"
        else -> "就绪"
    }

    LazyColumn(
        modifier = Modifier
            .fillMaxSize()
            .background(Brush.linearGradient(listOf(Cream, Color(0xFFE7F1EA)), start = Offset.Zero))
            .drawBehind {
                drawCircle(Color(0x22E9684A), radius = size.width * 0.42f, center = Offset(size.width, 0f))
                drawCircle(Color(0x22176B57), radius = size.width * 0.3f, center = Offset(0f, size.height * 0.55f))
            }
            .windowInsetsPadding(WindowInsets.safeDrawing)
            .padding(horizontal = 18.dp),
        contentPadding = androidx.compose.foundation.layout.PaddingValues(vertical = 24.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        item {
            Text("BACKUP / PRIVATE", color = Coral, style = MaterialTheme.typography.labelLarge, letterSpacing = 1.8.sp)
            Text("备份仓", style = MaterialTheme.typography.displaySmall, color = Ink)
            Text("HTTPS 加密传输，服务器保存可直接读取的原始文件；删除本地文件不会删除服务端副本。", color = Muted)
        }
        item {
            StatusCard(
                stage = stage,
                message = activeMessage,
                active = active,
                completed = completed,
                total = total,
                onBackup = { requestAction(PermissionAction.BACKUP) },
                onStop = {
                    BackupScheduler.stopCurrent(context, config)
                    val stopped = config.snapshot().copy(state = "idle", message = "已停止当前任务", currentItem = "")
                    config.saveSnapshot(stopped)
                    snapshot = stopped
                    notice = "当前任务已停止，自动备份将在下个周期继续"
                },
            )
        }
        item { MetricsRow(snapshot) }
        item {
            SectionCard(
                if (showingTrash) "服务器回收站" else "服务器时间线",
                "HTTPS 传输，服务器保存可直接恢复的原始媒体",
            ) {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Button(enabled = !libraryLoading, onClick = { refreshLibrary() }) {
                        Text(if (libraryLoading) "加载中" else "刷新")
                    }
                    OutlinedButton(onClick = {
                        showingTrash = !showingTrash
                        refreshLibrary(showingTrash)
                    }) { Text(if (showingTrash) "返回时间线" else "查看回收站") }
                    if (!showingTrash && remoteAssets.isNotEmpty()) {
                        OutlinedButton(onClick = {
                            scope.launch {
                                runCatching {
                                    withContext(Dispatchers.IO) {
                                        val api = BackupApi(config.serverUrl, config.bearerToken)
                                        remoteAssets.forEach { RemoteLibrary.restore(context, api, it) }
                                        remoteAssets.size
                                    }
                                }.onSuccess { notice = "已恢复 $it 个媒体项目" }
                                    .onFailure { notice = "批量恢复失败：${it.message}" }
                            }
                        }) { Text("全部恢复") }
                    }
                }
                Text(
                    if (duplicateGroupCount == 0) "未发现重复组" else "发现 $duplicateGroupCount 组重复内容，可将多余副本移入回收站",
                    color = Muted,
                    style = MaterialTheme.typography.bodyMedium,
                    modifier = Modifier.padding(top = 8.dp),
                )
                OutlinedTextField(
                    value = tagName,
                    onValueChange = { tagName = it },
                    modifier = Modifier.fillMaxWidth().padding(top = 8.dp),
                    label = { Text("要添加的标签") },
                    singleLine = true,
                )
                if (remoteAssets.isEmpty()) {
                    Text("尚未加载或没有媒体", color = Muted, modifier = Modifier.padding(top = 12.dp))
                }
                remoteAssets.take(12).forEach { asset ->
                    val primary = asset.resources.firstOrNull { it.role == "primary" }
                        ?: asset.resources.firstOrNull { it.role != "thumbnail" }
                    HorizontalDivider(color = Border, modifier = Modifier.padding(vertical = 8.dp))
                    remoteThumbnails[asset.id]?.let { thumbnail ->
                        Image(
                            bitmap = thumbnail.asImageBitmap(),
                            contentDescription = primary?.filename,
                            contentScale = ContentScale.Crop,
                            modifier = Modifier.size(width = 96.dp, height = 72.dp).clip(RoundedCornerShape(10.dp)),
                        )
                    }
                    Text(primary?.filename ?: "媒体资源", color = Ink, fontWeight = FontWeight.SemiBold)
                    Text(formatTime(asset.createdAtMs), color = Muted, style = MaterialTheme.typography.bodyMedium)
                    if (asset.tagNames.isNotEmpty()) {
                        Text(asset.tagNames.joinToString(" · "), color = Pine, style = MaterialTheme.typography.bodyMedium)
                    }
                    Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        if (!asset.trashed) {
                            TextButton(onClick = {
                                scope.launch {
                                    runCatching {
                                        withContext(Dispatchers.IO) {
                                            val api = BackupApi(config.serverUrl, config.bearerToken)
                                            RemoteLibrary.restore(context, api, asset)
                                        }
                                    }.onSuccess { notice = "已恢复 $it" }
                                        .onFailure { notice = "恢复失败：${it.message}" }
                                }
                            }) { Text("恢复到手机") }
                            TextButton(onClick = {
                                scope.launch {
                                    runCatching {
                                        withContext(Dispatchers.IO) {
                                            BackupApi(config.serverUrl, config.bearerToken)
                                                .updateAsset(asset.id, favorite = !asset.favorite)
                                        }
                                    }.onSuccess { refreshLibrary(false) }
                                        .onFailure { notice = "更新失败：${it.message}" }
                                }
                            }) { Text(if (asset.favorite) "取消收藏" else "收藏") }
                            TextButton(onClick = {
                                scope.launch {
                                    runCatching {
                                        withContext(Dispatchers.IO) {
                                            BackupApi(config.serverUrl, config.bearerToken)
                                                .updateAsset(asset.id, archived = !asset.archived)
                                        }
                                    }.onSuccess { refreshLibrary(false) }
                                        .onFailure { notice = "更新归档失败：${it.message}" }
                                }
                            }) { Text(if (asset.archived) "取消归档" else "归档") }
                            if (tagName.isNotBlank()) {
                                TextButton(onClick = {
                                    scope.launch {
                                        runCatching {
                                            withContext(Dispatchers.IO) {
                                                val api = BackupApi(config.serverUrl, config.bearerToken)
                                                val normalized = tagName.trim()
                                                val tags = api.listTags()
                                                var tagId: String? = null
                                                for (position in 0 until tags.length()) {
                                                    val tag = tags.getJSONObject(position)
                                                    if (tag.getString("name").equals(normalized, ignoreCase = true)) {
                                                        tagId = tag.getString("tag_id")
                                                        break
                                                    }
                                                }
                                                if (tagId == null) tagId = api.createTag(normalized).getString("tag_id")
                                                api.addTagAsset(requireNotNull(tagId), asset.id)
                                            }
                                        }.onSuccess { refreshLibrary(false) }
                                            .onFailure { notice = "添加标签失败：${it.message}" }
                                    }
                                }) { Text("加标签") }
                            }
                            TextButton(onClick = {
                                scope.launch {
                                    runCatching {
                                        withContext(Dispatchers.IO) {
                                            BackupApi(config.serverUrl, config.bearerToken).trashAsset(asset.id)
                                        }
                                    }.onSuccess { refreshLibrary(false) }
                                        .onFailure { notice = "移入回收站失败：${it.message}" }
                                }
                            }) { Text("回收站", color = Coral) }
                        } else {
                            TextButton(onClick = {
                                scope.launch {
                                    runCatching {
                                        withContext(Dispatchers.IO) {
                                            BackupApi(config.serverUrl, config.bearerToken).restoreAsset(asset.id)
                                        }
                                    }.onSuccess { refreshLibrary(true) }
                                        .onFailure { notice = "撤销删除失败：${it.message}" }
                                }
                            }) { Text("撤销删除") }
                        }
                    }
                }
            }
        }
        item {
            SectionCard("自动备份", "由 Android 在满足条件时持续检查新增内容") {
                SettingSwitch(
                    title = "自动备份新项目",
                    description = "至少每 6 小时检查一次，重启后仍会继续",
                    checked = autoBackup,
                    onCheckedChange = { autoBackup = it },
                )
                HorizontalDivider(color = Border)
                SettingSwitch(
                    title = "仅使用 Wi-Fi",
                    description = "避免消耗移动数据流量",
                    checked = wifiOnly,
                    onCheckedChange = { wifiOnly = it },
                )
                HorizontalDivider(color = Border)
                SettingSwitch(
                    title = "仅在充电时",
                    description = "适合首次备份大量照片和视频",
                    checked = chargingOnly,
                    onCheckedChange = { chargingOnly = it },
                )
            }
        }
        item {
            SectionCard("备份内容", "控制媒体类型和设备文件夹范围") {
                SettingSwitch("照片", "按原始文件备份，不压缩", backupPhotos) { backupPhotos = it }
                HorizontalDivider(color = Border)
                SettingSwitch("视频", "大文件自动分块并支持断点续传", backupVideos) { backupVideos = it }
                HorizontalDivider(color = Border)
                SettingSwitch(
                    "仅相机文件夹",
                    if (cameraOnly) "只备份 DCIM；下方相册选择暂不生效" else "按下方选择备份设备相册",
                    cameraOnly,
                ) { cameraOnly = it }
                if (!cameraOnly && deviceAlbums.isNotEmpty()) {
                    HorizontalDivider(color = Border)
                    Text("设备相册", color = Ink, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(top = 10.dp))
                    deviceAlbums.take(24).forEach { album ->
                        SettingSwitch(
                            album.name,
                            "${album.count} 项；关闭即排除",
                            album.id in selectedAlbumIds,
                        ) { enabled ->
                            selectedAlbumIds = if (enabled) selectedAlbumIds + album.id else selectedAlbumIds - album.id
                        }
                    }
                }
            }
        }
        item {
            SectionCard("私有服务器", "设备凭据加密保存在 Android Keystore 中") {
                OutlinedTextField(
                    value = serverUrl,
                    onValueChange = { serverUrl = it },
                    modifier = Modifier.fillMaxWidth(),
                    label = { Text("服务器地址") },
                    placeholder = { Text("https://photos.example.com") },
                    singleLine = true,
                )
                Spacer(Modifier.height(10.dp))
                OutlinedTextField(
                    value = username,
                    onValueChange = { username = it },
                    modifier = Modifier.fillMaxWidth(),
                    label = { Text("登录账号") },
                    singleLine = true,
                )
                Spacer(Modifier.height(10.dp))
                OutlinedTextField(
                    value = password,
                    onValueChange = { password = it },
                    modifier = Modifier.fillMaxWidth(),
                    label = { Text("登录密码") },
                    visualTransformation = PasswordVisualTransformation(),
                    singleLine = true,
                )
                Spacer(Modifier.height(14.dp))
                Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    Button(onClick = { requestAction(PermissionAction.SAVE) }) { Text("保存并应用") }
                    OutlinedButton(
                        enabled = !checkingConnection,
                        onClick = {
                            if (!persistSettings()) return@OutlinedButton
                            checkingConnection = true
                            connectionState = "检测中"
                            scope.launch {
                                connectionState = withContext(Dispatchers.IO) {
                                    runCatching { BackupApi(config.serverUrl, config.bearerToken).health() }
                                        .fold(onSuccess = { "连接正常" }, onFailure = { "连接失败：${it.message}" })
                                }
                                checkingConnection = false
                            }
                        },
                    ) { Text(if (checkingConnection) "检测中" else "检测连接") }
                }
                Spacer(Modifier.height(10.dp))
                Text(connectionState, color = if (connectionState == "连接正常") Pine else Muted)
            }
        }
        item {
            SectionCard("最近活动", "失败项目保留在本地队列并自动重试") {
                ActivityLine("最近运行", formatTime(snapshot.lastRunAt))
                ActivityLine("最近成功", formatTime(snapshot.lastSuccessAt))
                ActivityLine("当前状态", snapshot.message)
                if (snapshot.currentItem.isNotBlank()) ActivityLine("正在处理", snapshot.currentItem)
                if (snapshot.failed > 0) ActivityLine("失败次数", snapshot.failed.toString(), Coral)
            }
        }
        if (notice.isNotBlank()) {
            item {
                Surface(color = Ink, contentColor = Color.White, shape = RoundedCornerShape(14.dp)) {
                    Row(
                        modifier = Modifier.fillMaxWidth().padding(14.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text(notice, modifier = Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium)
                        TextButton(onClick = { notice = "" }) { Text("关闭", color = Mint) }
                    }
                }
            }
        }
    }
}

@Composable
private fun StatusCard(
    stage: String,
    message: String,
    active: Boolean,
    completed: Int,
    total: Int,
    onBackup: () -> Unit,
    onStop: () -> Unit,
) {
    Card(
        shape = RoundedCornerShape(28.dp),
        colors = CardDefaults.cardColors(containerColor = Ink),
        elevation = CardDefaults.cardElevation(defaultElevation = 6.dp),
    ) {
        Column(Modifier.padding(22.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.size(11.dp).background(if (active) Coral else Mint, CircleShape))
                Spacer(Modifier.width(9.dp))
                Text(stage.uppercase(), color = Mint, fontWeight = FontWeight.Bold, letterSpacing = 1.2.sp)
            }
            Text(
                message,
                color = Color.White,
                style = MaterialTheme.typography.headlineMedium,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
            if (active) {
                if (total > 0) {
                    LinearProgressIndicator(
                        progress = { (completed.toFloat() / total.toFloat()).coerceIn(0f, 1f) },
                        modifier = Modifier.fillMaxWidth().height(7.dp),
                        color = Coral,
                        trackColor = Color(0xFF355047),
                    )
                    Text("$completed / $total", color = Mint)
                } else {
                    LinearProgressIndicator(modifier = Modifier.fillMaxWidth(), color = Coral, trackColor = Color(0xFF355047))
                }
            }
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Button(
                    onClick = onBackup,
                    enabled = !active,
                    colors = ButtonDefaults.buttonColors(containerColor = Coral, disabledContainerColor = Color(0xFF53665F)),
                ) { Text(if (active) "备份进行中" else "立即备份") }
                if (active) {
                    OutlinedButton(onClick = onStop) { Text("停止", color = Color.White) }
                }
            }
        }
    }
}

@Composable
private fun MetricsRow(snapshot: BackupSnapshot) {
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        MetricCard("发现", snapshot.discovered.toString(), Modifier.weight(1f))
        MetricCard("完成", snapshot.completed.toString(), Modifier.weight(1f))
        MetricCard("上传", formatBytes(snapshot.uploadedBytes), Modifier.weight(1f))
    }
}

@Composable
private fun MetricCard(label: String, value: String, modifier: Modifier) {
    Surface(modifier = modifier, shape = RoundedCornerShape(18.dp), color = Paper) {
        Column(Modifier.padding(horizontal = 13.dp, vertical = 15.dp)) {
            Text(label, color = Muted, style = MaterialTheme.typography.bodyMedium)
            Text(value, color = Ink, style = MaterialTheme.typography.titleLarge, maxLines = 1)
        }
    }
}

@Composable
private fun SectionCard(title: String, subtitle: String, content: @Composable ColumnScope.() -> Unit) {
    Card(shape = RoundedCornerShape(24.dp), colors = CardDefaults.cardColors(containerColor = Paper)) {
        Column(Modifier.padding(18.dp)) {
            Text(title, style = MaterialTheme.typography.titleLarge, color = Ink)
            Text(subtitle, color = Muted, style = MaterialTheme.typography.bodyMedium)
            Spacer(Modifier.height(16.dp))
            content()
        }
    }
}

@Composable
private fun SettingSwitch(title: String, description: String, checked: Boolean, onCheckedChange: (Boolean) -> Unit) {
    Row(
        modifier = Modifier.fillMaxWidth().padding(vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f)) {
            Text(title, color = Ink, fontWeight = FontWeight.SemiBold)
            Text(description, color = Muted, style = MaterialTheme.typography.bodyMedium)
        }
        Spacer(Modifier.width(12.dp))
        Switch(checked = checked, onCheckedChange = onCheckedChange)
    }
}

@Composable
private fun ActivityLine(label: String, value: String, valueColor: Color = Ink) {
    Row(Modifier.fillMaxWidth().padding(vertical = 6.dp), verticalAlignment = Alignment.Top) {
        Text(label, color = Muted, modifier = Modifier.width(82.dp), style = MaterialTheme.typography.bodyMedium)
        Text(value, color = valueColor, modifier = Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium)
    }
}

private fun mediaPermissions(includePhotos: Boolean, includeVideos: Boolean): Array<String> {
    val permissions = mutableListOf<String>()
    if (Build.VERSION.SDK_INT >= 33) {
        if (includePhotos) permissions += Manifest.permission.READ_MEDIA_IMAGES
        if (includeVideos) permissions += Manifest.permission.READ_MEDIA_VIDEO
        if (Build.VERSION.SDK_INT >= 34) permissions += Manifest.permission.READ_MEDIA_VISUAL_USER_SELECTED
        permissions += Manifest.permission.POST_NOTIFICATIONS
    } else {
        permissions += Manifest.permission.READ_EXTERNAL_STORAGE
    }
    return permissions.toTypedArray()
}

private fun hasMediaAccess(context: Context, includePhotos: Boolean, includeVideos: Boolean): Boolean {
    if (Build.VERSION.SDK_INT < 33) {
        return ContextCompat.checkSelfPermission(context, Manifest.permission.READ_EXTERNAL_STORAGE) ==
            PackageManager.PERMISSION_GRANTED
    }
    val partial = Build.VERSION.SDK_INT >= 34 && ContextCompat.checkSelfPermission(
        context,
        Manifest.permission.READ_MEDIA_VISUAL_USER_SELECTED,
    ) == PackageManager.PERMISSION_GRANTED
    val photosAllowed = !includePhotos || partial || ContextCompat.checkSelfPermission(
        context,
        Manifest.permission.READ_MEDIA_IMAGES,
    ) == PackageManager.PERMISSION_GRANTED
    val videosAllowed = !includeVideos || partial || ContextCompat.checkSelfPermission(
        context,
        Manifest.permission.READ_MEDIA_VIDEO,
    ) == PackageManager.PERMISSION_GRANTED
    return photosAllowed && videosAllowed
}

private fun formatTime(timestamp: Long): String = if (timestamp <= 0L) {
    "暂无"
} else {
    DateFormat.getDateTimeInstance(DateFormat.MEDIUM, DateFormat.SHORT).format(Date(timestamp))
}

private fun formatBytes(bytes: Long): String = when {
    bytes >= 1024L * 1024L * 1024L -> String.format("%.1f GB", bytes / (1024.0 * 1024.0 * 1024.0))
    bytes >= 1024L * 1024L -> String.format("%.1f MB", bytes / (1024.0 * 1024.0))
    bytes >= 1024L -> String.format("%.1f KB", bytes / 1024.0)
    else -> "$bytes B"
}
