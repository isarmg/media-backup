package com.example.mediabackup.v02

import android.content.ContentValues
import android.content.Context
import android.os.Build
import android.provider.MediaStore
import org.json.JSONObject

data class RemoteResource(
    val id: String,
    val role: String,
    val filename: String,
    val mimeType: String,
    val contentPath: String,
)

data class RemoteAsset(
    val id: String,
    val createdAtMs: Long,
    val favorite: Boolean,
    val archived: Boolean,
    val trashed: Boolean,
    val tagNames: List<String>,
    val resources: List<RemoteResource>,
)

object RemoteLibrary {
    fun advanceSync(api: BackupApi, initialSequence: Long): Long {
        var sequence = initialSequence
        var more: Boolean
        do {
            val page = api.sync(sequence)
            sequence = page.getLong("next_sequence")
            more = page.getBoolean("has_more")
        } while (more)
        return sequence
    }

    fun timeline(api: BackupApi, trashed: Boolean = false): List<RemoteAsset> {
        val result = mutableListOf<RemoteAsset>()
        var cursor: String? = null
        do {
            val page = api.timeline(cursor = cursor, trashed = trashed)
            val items = page.getJSONArray("items")
            for (position in 0 until items.length()) result += parseAsset(items.getJSONObject(position))
            val next = page.optString("next_cursor").takeIf { it.isNotBlank() && it != "null" }
            check(next == null || next != cursor) { "服务器返回了无效的分页游标" }
            cursor = next
        } while (cursor != null)
        return result
    }

    fun restore(context: Context, api: BackupApi, asset: RemoteAsset): String {
        val resource = asset.resources.firstOrNull { it.role == "primary" }
            ?: asset.resources.firstOrNull { it.role != "thumbnail" }
            ?: error("该资产没有可恢复的原始资源")
        val collection = if (resource.mimeType.startsWith("video/")) {
            MediaStore.Video.Media.EXTERNAL_CONTENT_URI
        } else {
            MediaStore.Images.Media.EXTERNAL_CONTENT_URI
        }
        val values = ContentValues().apply {
            put(MediaStore.MediaColumns.DISPLAY_NAME, resource.filename)
            put(MediaStore.MediaColumns.MIME_TYPE, resource.mimeType)
            if (Build.VERSION.SDK_INT >= 29) {
                put(MediaStore.MediaColumns.RELATIVE_PATH, "Pictures/MediaBackup Restored")
                put(MediaStore.MediaColumns.IS_PENDING, 1)
            }
        }
        val uri = context.contentResolver.insert(collection, values) ?: error("无法创建恢复文件")
        try {
            context.contentResolver.openOutputStream(uri)?.use { api.download(resource.contentPath, it) }
                ?: error("无法打开恢复文件")
            if (Build.VERSION.SDK_INT >= 29) {
                context.contentResolver.update(uri, ContentValues().apply { put(MediaStore.MediaColumns.IS_PENDING, 0) }, null, null)
            }
            return resource.filename
        } catch (error: Exception) {
            context.contentResolver.delete(uri, null, null)
            throw error
        }
    }

    fun thumbnail(api: BackupApi, asset: RemoteAsset): ByteArray? {
        val resource = asset.resources.firstOrNull { it.role == "thumbnail" }
            ?: return null
        return api.downloadBytes(resource.contentPath)
    }

    private fun parseAsset(value: JSONObject): RemoteAsset {
        val resources = value.getJSONArray("resources")
        return RemoteAsset(
            id = value.getString("asset_id"),
            createdAtMs = value.getLong("source_created_at_ms"),
            favorite = value.getBoolean("favorite"),
            archived = value.getBoolean("archived"),
            trashed = !value.isNull("trashed_at_ms"),
            tagNames = buildList {
                val tags = value.optJSONArray("tag_names") ?: org.json.JSONArray()
                for (position in 0 until tags.length()) add(tags.getString(position))
            },
            resources = buildList {
                for (position in 0 until resources.length()) {
                    val resource = resources.getJSONObject(position)
                    check(resource.getString("storage_encoding") == "plain-v1") {
                        "服务器返回了非当前存储协议"
                    }
                    add(
                        RemoteResource(
                            id = resource.getString("resource_id"),
                            role = resource.getString("role"),
                            filename = resource.getString("filename"),
                            mimeType = resource.getString("mime_type"),
                            contentPath = resource.getString("content_path"),
                        ),
                    )
                }
            },
        )
    }
}
