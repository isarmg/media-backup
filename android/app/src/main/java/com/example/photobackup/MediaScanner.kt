package com.example.photobackup

import android.content.ContentUris
import android.content.Context
import android.os.Build
import android.provider.MediaStore
import org.json.JSONObject
import java.io.File
import java.util.UUID

data class ScanOptions(
    val includePhotos: Boolean,
    val includeVideos: Boolean,
    val cameraOnly: Boolean,
    val maxItems: Int = 40,
)

data class ScanResult(
    val discovered: Int,
    val discoveredBytes: Long,
    val skipped: Int,
    val limitReached: Boolean,
)

class MediaScanner(private val context: Context) {
    fun scan(handle: Long, stagingRoot: File, options: ScanOptions): ScanResult {
        val sourceDir = File(stagingRoot, "sources").also { it.mkdirs() }
        var discovered = 0
        var discoveredBytes = 0L
        var skipped = 0
        if (options.includePhotos) {
            val result = scanCollection(
                handle,
                MediaStore.Images.Media.EXTERNAL_CONTENT_URI,
                "photo",
                sourceDir,
                options,
                options.maxItems,
            )
            discovered += result.discovered
            discoveredBytes += result.bytes
            skipped += result.skipped
        }
        if (options.includeVideos && discovered < options.maxItems) {
            val result = scanCollection(
                handle,
                MediaStore.Video.Media.EXTERNAL_CONTENT_URI,
                "video",
                sourceDir,
                options,
                options.maxItems - discovered,
            )
            discovered += result.discovered
            discoveredBytes += result.bytes
            skipped += result.skipped
        }
        return ScanResult(discovered, discoveredBytes, skipped, discovered >= options.maxItems)
    }

    private fun scanCollection(
        handle: Long,
        collection: android.net.Uri,
        mediaKind: String,
        sourceDir: File,
        options: ScanOptions,
        limit: Int,
    ): CollectionResult {
        val projection = mutableListOf(
            MediaStore.MediaColumns._ID,
            MediaStore.MediaColumns.DISPLAY_NAME,
            MediaStore.MediaColumns.MIME_TYPE,
            MediaStore.MediaColumns.SIZE,
            MediaStore.MediaColumns.DATE_MODIFIED,
            MediaStore.MediaColumns.DATE_ADDED,
            MediaStore.Images.ImageColumns.BUCKET_DISPLAY_NAME,
        )
        if (Build.VERSION.SDK_INT >= 29) projection += MediaStore.MediaColumns.RELATIVE_PATH
        var queued = 0
        var bytes = 0L
        var skipped = 0
        try {
            context.contentResolver.query(
                collection,
                projection.toTypedArray(),
                null,
                null,
                "${MediaStore.MediaColumns.DATE_ADDED} ASC",
            )?.use { cursor ->
            val idColumn = cursor.getColumnIndexOrThrow(MediaStore.MediaColumns._ID)
            val nameColumn = cursor.getColumnIndexOrThrow(MediaStore.MediaColumns.DISPLAY_NAME)
            val mimeColumn = cursor.getColumnIndexOrThrow(MediaStore.MediaColumns.MIME_TYPE)
            val modifiedColumn = cursor.getColumnIndexOrThrow(MediaStore.MediaColumns.DATE_MODIFIED)
            val createdColumn = cursor.getColumnIndexOrThrow(MediaStore.MediaColumns.DATE_ADDED)
            val bucketColumn = cursor.getColumnIndex(MediaStore.Images.ImageColumns.BUCKET_DISPLAY_NAME)
            val relativePathColumn = if (Build.VERSION.SDK_INT >= 29) {
                cursor.getColumnIndex(MediaStore.MediaColumns.RELATIVE_PATH)
            } else {
                -1
            }
            while (cursor.moveToNext()) {
                if (queued >= limit) break
                val bucketName = if (bucketColumn >= 0) cursor.getString(bucketColumn) else null
                val relativePath = if (relativePathColumn >= 0) cursor.getString(relativePathColumn) else null
                if (options.cameraOnly && !isCameraMedia(bucketName, relativePath)) continue
                val uri = ContentUris.withAppendedId(collection, cursor.getLong(idColumn))
                val sourceId = uri.toString()
                val modifiedMs = cursor.getLong(modifiedColumn) * 1000L
                if (!NativeBridge.nativeNeeds(handle, sourceId, sourceId, modifiedMs)) continue
                val output = File(sourceDir, UUID.randomUUID().toString())
                try {
                    context.contentResolver.openInputStream(uri)?.use { input ->
                        output.outputStream().use { target -> input.copyTo(target) }
                    } ?: continue
                    val metadata = JSONObject()
                        .put("android_uri", sourceId)
                        .put("display_name", cursor.getString(nameColumn))
                        .put("bucket_name", bucketName ?: JSONObject.NULL)
                        .put("relative_path", relativePath ?: JSONObject.NULL)
                        .put("date_modified_ms", modifiedMs)
                    val enqueue = JSONObject()
                        .put("source_asset_id", sourceId)
                        .put("source_resource_id", sourceId)
                        .put("media_kind", mediaKind)
                        .put("role", "primary")
                        .put("file_path", output.absolutePath)
                        .put("filename", cursor.getString(nameColumn) ?: "media")
                        .put("mime_type", cursor.getString(mimeColumn) ?: "application/octet-stream")
                        .put("source_created_at_ms", cursor.getLong(createdColumn) * 1000L)
                        .put("modified_ms", modifiedMs)
                        .put("source_size", output.length())
                        .put("metadata_json", metadata.toString())
                        .put("remove_source_after_prepare", true)
                    val result = JSONObject(NativeBridge.nativeEnqueue(handle, enqueue.toString()))
                    if (!result.getBoolean("ok")) error(result.optString("error", "Rust Agent enqueue failed"))
                    queued++
                    bytes += output.length()
                } catch (error: Exception) {
                    output.delete()
                    skipped++
                }
            }
        }
        } catch (error: SecurityException) {
            skipped++
        }
        return CollectionResult(queued, bytes, skipped)
    }

    private fun isCameraMedia(bucketName: String?, relativePath: String?): Boolean {
        if (relativePath?.replace('\\', '/')?.startsWith("DCIM/", ignoreCase = true) == true) return true
        return bucketName.equals("Camera", ignoreCase = true)
    }

    private data class CollectionResult(val discovered: Int, val bytes: Long, val skipped: Int)
}
