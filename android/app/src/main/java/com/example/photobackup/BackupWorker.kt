package com.example.photobackup.v02

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat
import androidx.work.CoroutineWorker
import androidx.work.ForegroundInfo
import androidx.work.WorkerParameters
import androidx.work.workDataOf
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.coroutines.sync.Mutex
import org.json.JSONObject
import java.io.File

class BackupWorker(context: Context, parameters: WorkerParameters) : CoroutineWorker(context, parameters) {
    override suspend fun doWork(): Result {
        backupMutex.lock()
        return try {
            withContext(Dispatchers.IO) { runBackup() }
        } finally {
            backupMutex.unlock()
        }
    }

    private suspend fun runBackup(): Result {
        val config = SecureConfig(applicationContext)
        val startedAt = System.currentTimeMillis()
        var snapshot = BackupSnapshot(
            state = "running",
            message = "正在准备备份",
            lastRunAt = startedAt,
        )
        publish(config, snapshot, "准备", 0, 0)
        if (config.serverUrl.isBlank() || config.username.isBlank() || config.password.isBlank()) {
            snapshot = snapshot.copy(state = "error", message = "请先配置服务器地址、账号和密码", failed = 1)
            config.saveSnapshot(snapshot)
            return Result.failure(workDataOf(OUTPUT_MESSAGE to snapshot.message))
        }
        if (!config.backupPhotos && !config.backupVideos) {
            snapshot = snapshot.copy(state = "error", message = "至少选择照片或视频中的一种", failed = 1)
            config.saveSnapshot(snapshot)
            return Result.failure(workDataOf(OUTPUT_MESSAGE to snapshot.message))
        }
        if (!hasMediaAccess(config)) {
            snapshot = snapshot.copy(state = "error", message = "需要照片和视频访问权限", failed = 1)
            config.saveSnapshot(snapshot)
            return Result.failure(workDataOf(OUTPUT_MESSAGE to snapshot.message))
        }
        var token = config.bearerToken
        val api = BackupApi(config.serverUrl, token)
        return try {
            if (token.isBlank()) {
                publish(config, snapshot.copy(message = "正在注册此设备"), "连接", 0, 0)
                token = api.bootstrap(config.username, config.password, Build.MODEL)
                config.bearerToken = token
            }
            val nativeConfig = MobileContractV02.putIdentity(JSONObject())
                .put("part_size", 16 * 1024 * 1024)
            val handle = NativeBridgeV02.openV02R1(
                applicationContext.getDatabasePath(MobileContractV02.DATABASE_FILENAME).absolutePath,
                nativeConfig.toString(),
            )
            if (handle == 0L) error("无法打开 Rust Agent")
            try {
                val stagingRoot = File(
                    applicationContext.filesDir,
                    MobileContractV02.STAGING_DIRECTORY,
                ).also { it.mkdirs() }
                publish(config, snapshot.copy(message = "正在扫描媒体库"), "扫描", 0, 0)
                val selectedAlbumIds = if (!config.cameraOnly && !config.albumSelectionConfigured) {
                    DeviceAlbums.list(applicationContext).mapTo(mutableSetOf()) { it.id }
                        .also { config.selectedAlbumIds = it }
                } else {
                    config.selectedAlbumIds
                }
                val scan = MediaScanner(applicationContext).scan(
                    handle,
                    stagingRoot,
                    ScanOptions(
                        includePhotos = config.backupPhotos,
                        includeVideos = config.backupVideos,
                        cameraOnly = config.cameraOnly,
                        selectedAlbumIds = selectedAlbumIds,
                        maxItems = MAX_SCAN_ITEMS,
                    ),
                )
                snapshot = snapshot.copy(
                    message = if (scan.discovered == 0) "正在检查待续传项目" else "发现 ${scan.discovered} 个待备份项目",
                    discovered = scan.discovered,
                )
                publish(config, snapshot, "上传", 0, scan.discovered)
                var queueDrained = false
                while (snapshot.completed < MAX_UPLOAD_ITEMS) {
                    if (isStopped) {
                        config.saveSnapshot(snapshot.copy(state = "idle", message = "备份已停止", currentItem = ""))
                        return Result.failure(workDataOf(OUTPUT_MESSAGE to "备份已停止"))
                    }
                    val envelope = MobileContractV02.requireEnvelope(
                        NativeBridgeV02.nextV02R1(handle, stagingRoot.absolutePath),
                    )
                    if (envelope.isNull("value")) {
                        queueDrained = true
                        break
                    }
                    val job = envelope.getJSONObject("value")
                    MobileContractV02.requireIdentity(job)
                    val filename = job.optJSONObject("request")?.optString("filename", "媒体文件") ?: "媒体文件"
                    snapshot = snapshot.copy(message = "正在备份 $filename", currentItem = filename)
                    publish(config, snapshot, "上传", snapshot.completed, maxOf(scan.discovered, 1))
                    val uploadedBytes = uploadJob(api, handle, job)
                    snapshot = snapshot.copy(
                        completed = snapshot.completed + 1,
                        uploadedBytes = snapshot.uploadedBytes + uploadedBytes,
                        message = "已完成 ${snapshot.completed + 1} 项",
                        currentItem = "",
                    )
                    publish(config, snapshot, "上传", snapshot.completed, maxOf(scan.discovered, snapshot.completed))
                }
                for ((albumId, album) in scan.albums) {
                    api.syncAlbum(
                        albumId,
                        album.name,
                        album.sourceAssetIds,
                        replaceMembers = !scan.limitReached,
                    )
                }
                if (scan.limitReached || !queueDrained) {
                    snapshot = snapshot.copy(
                        state = "waiting",
                        message = "本批已完成，系统将继续处理剩余项目",
                        currentItem = "",
                    )
                    config.saveSnapshot(snapshot)
                    return Result.retry()
                }
            } finally {
                NativeBridgeV02.closeV02R1(handle)
            }
            snapshot = snapshot.copy(
                state = "success",
                message = if (snapshot.completed == 0) "所有项目均已备份" else "本次成功备份 ${snapshot.completed} 项",
                lastSuccessAt = System.currentTimeMillis(),
                currentItem = "",
            )
            config.saveSnapshot(snapshot)
            Result.success(
                workDataOf(
                    OUTPUT_MESSAGE to snapshot.message,
                    PROGRESS_COMPLETED to snapshot.completed,
                    PROGRESS_TOTAL to snapshot.discovered,
                ),
            )
        } catch (error: Exception) {
            snapshot = snapshot.copy(
                state = "error",
                message = error.message?.take(180) ?: "备份失败，等待系统重试",
                failed = snapshot.failed + 1,
                currentItem = "",
            )
            config.saveSnapshot(snapshot)
            Result.retry()
        }
    }

    private fun uploadJob(api: BackupApi, handle: Long, job: JSONObject): Long {
        val jobId = job.getString("job_id")
        try {
            val create = api.createUpload(job.getJSONObject("request").toString())
            if (create.getString("disposition") == "complete") {
                MobileContractV02.requireEnvelope(NativeBridgeV02.markCompleteV02R1(handle, jobId))
                return 0L
            }
            val uploadId = create.getString("upload_id")
            MobileContractV02.requireEnvelope(NativeBridgeV02.markUploadV02R1(handle, jobId, uploadId))
            val files = mutableMapOf<Int, File>()
            val localParts = job.getJSONArray("local_parts")
            for (position in 0 until localParts.length()) {
                val part = localParts.getJSONObject(position)
                files[part.getInt("index")] = File(part.getString("path"))
            }
            val missing = create.getJSONArray("missing_parts")
            var uploadedBytes = 0L
            for (position in 0 until missing.length()) {
                val index = missing.getInt(position)
                val file = files[index] ?: error("缺少本地分块 $index")
                api.uploadPart(uploadId, index, file)
                uploadedBytes += file.length()
                MobileContractV02.requireEnvelope(NativeBridgeV02.markPartV02R1(handle, jobId, index))
            }
            api.complete(uploadId)
            MobileContractV02.requireEnvelope(NativeBridgeV02.markCompleteV02R1(handle, jobId))
            return uploadedBytes
        } catch (error: Exception) {
            MobileContractV02.requireEnvelope(
                NativeBridgeV02.markFailedV02R1(handle, jobId, error.message ?: "上传失败", true),
            )
            throw error
        }
    }

    private suspend fun publish(
        config: SecureConfig,
        snapshot: BackupSnapshot,
        stage: String,
        completed: Int,
        total: Int,
    ) {
        config.saveSnapshot(snapshot)
        setProgress(
            workDataOf(
                PROGRESS_STAGE to stage,
                PROGRESS_MESSAGE to snapshot.message,
                PROGRESS_COMPLETED to completed,
                PROGRESS_TOTAL to total,
                PROGRESS_ITEM to snapshot.currentItem,
            ),
        )
        setForeground(createForegroundInfo(snapshot.message, completed, total))
    }

    private fun hasMediaAccess(config: SecureConfig): Boolean {
        if (Build.VERSION.SDK_INT < 33) {
            return ContextCompat.checkSelfPermission(
                applicationContext,
                android.Manifest.permission.READ_EXTERNAL_STORAGE,
            ) == PackageManager.PERMISSION_GRANTED
        }
        val partial = Build.VERSION.SDK_INT >= 34 && ContextCompat.checkSelfPermission(
            applicationContext,
            android.Manifest.permission.READ_MEDIA_VISUAL_USER_SELECTED,
        ) == PackageManager.PERMISSION_GRANTED
        val photosAllowed = !config.backupPhotos || partial || ContextCompat.checkSelfPermission(
            applicationContext,
            android.Manifest.permission.READ_MEDIA_IMAGES,
        ) == PackageManager.PERMISSION_GRANTED
        val videosAllowed = !config.backupVideos || partial || ContextCompat.checkSelfPermission(
            applicationContext,
            android.Manifest.permission.READ_MEDIA_VIDEO,
        ) == PackageManager.PERMISSION_GRANTED
        return photosAllowed && videosAllowed
    }

    private fun createForegroundInfo(message: String, completed: Int, total: Int): ForegroundInfo {
        val manager = applicationContext.getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(NotificationChannel(CHANNEL_ID, "照片备份", NotificationManager.IMPORTANCE_LOW))
        val contentIntent = PendingIntent.getActivity(
            applicationContext,
            0,
            Intent(applicationContext, MainActivity::class.java),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val notification = NotificationCompat.Builder(applicationContext, CHANNEL_ID)
            .setSmallIcon(android.R.drawable.stat_sys_upload)
            .setContentTitle("照片备份")
            .setContentText(message)
            .setContentIntent(contentIntent)
            .setOnlyAlertOnce(true)
            .setOngoing(true)
            .setProgress(maxOf(total, 1), completed.coerceAtMost(maxOf(total, 1)), total <= 0)
            .build()
        return if (Build.VERSION.SDK_INT >= 29) {
            ForegroundInfo(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        } else {
            ForegroundInfo(NOTIFICATION_ID, notification)
        }
    }

    companion object {
        const val PROGRESS_STAGE = "stage"
        const val PROGRESS_MESSAGE = "message"
        const val PROGRESS_COMPLETED = "completed"
        const val PROGRESS_TOTAL = "total"
        const val PROGRESS_ITEM = "item"
        const val OUTPUT_MESSAGE = "result_message"

        private const val CHANNEL_ID = MobileContractV02.NOTIFICATION_CHANNEL
        private const val NOTIFICATION_ID = 4102
        private const val MAX_SCAN_ITEMS = 40
        private const val MAX_UPLOAD_ITEMS = 60
        private val backupMutex = Mutex()
    }
}
