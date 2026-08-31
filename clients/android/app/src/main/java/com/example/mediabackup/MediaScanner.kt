package com.example.mediabackup.v02

import android.content.ContentUris
import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.media.ThumbnailUtils
import android.os.Build
import android.provider.MediaStore
import org.json.JSONObject
import java.io.File
import java.util.UUID

data class ScanOptions(
    val includePhotos: Boolean,
    val includeVideos: Boolean,
    val cameraOnly: Boolean,
    val selectedAlbumIds: Set<String> = emptySet(),
    val maxItems: Int = 40,
)

data class ScanResult(
    val discovered: Int,
    val discoveredBytes: Long,
    val skipped: Int,
    val limitReached: Boolean,
    val albums: Map<String, ScannedAlbum>,
)

data class ScannedAlbum(val name: String, val sourceAssetIds: Set<String>)

class MediaScanner(private val context: Context) {
    fun scan(handle: Long, stagingRoot: File, options: ScanOptions): ScanResult {
        val sourceDir = File(stagingRoot, "sources").also { it.mkdirs() }
        var discovered = 0
        var discoveredBytes = 0L
        var skipped = 0
        val albums = mutableMapOf<String, MutableAlbum>()
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
            mergeAlbums(albums, result.albums)
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
            mergeAlbums(albums, result.albums)
        }
        return ScanResult(
            discovered,
            discoveredBytes,
            skipped,
            discovered >= options.maxItems,
            albums.mapValues { ScannedAlbum(it.value.name, it.value.sourceAssetIds) },
        )
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
            MediaStore.Images.ImageColumns.BUCKET_ID,
        )
        if (Build.VERSION.SDK_INT >= 29) projection += MediaStore.MediaColumns.RELATIVE_PATH
        var queued = 0
        var bytes = 0L
        var skipped = 0
        val albums = mutableMapOf<String, MutableAlbum>()
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
            val bucketIdColumn = cursor.getColumnIndex(MediaStore.Images.ImageColumns.BUCKET_ID)
            val relativePathColumn = if (Build.VERSION.SDK_INT >= 29) {
                cursor.getColumnIndex(MediaStore.MediaColumns.RELATIVE_PATH)
            } else {
                -1
            }
            while (cursor.moveToNext()) {
                if (queued >= limit) break
                val bucketName = if (bucketColumn >= 0) cursor.getString(bucketColumn) else null
                val bucketId = if (bucketIdColumn >= 0) cursor.getString(bucketIdColumn) else null
                val relativePath = if (relativePathColumn >= 0) cursor.getString(relativePathColumn) else null
                if (options.cameraOnly && !isCameraMedia(bucketName, relativePath)) continue
                if (!options.cameraOnly && bucketId !in options.selectedAlbumIds) continue
                val uri = ContentUris.withAppendedId(collection, cursor.getLong(idColumn))
                val sourceId = uri.toString()
                val modifiedMs = cursor.getLong(modifiedColumn) * 1000L
                if (bucketId != null && bucketName != null) {
                    albums.getOrPut(bucketId) { MutableAlbum(bucketName) }.sourceAssetIds += sourceId
                }
                val thumbnailId = "$sourceId#thumbnail-v1"
                val needsPrimary = NativeBridgeV02.needsV02R1(handle, sourceId, sourceId, modifiedMs)
                val needsThumbnail = NativeBridgeV02.needsV02R1(handle, sourceId, thumbnailId, modifiedMs)
                if (!needsPrimary && !needsThumbnail) continue
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
                    if (needsPrimary) {
                        enqueue(
                            handle,
                            sourceId,
                            sourceId,
                            mediaKind,
                            "primary",
                            output,
                            cursor.getString(nameColumn) ?: "media",
                            cursor.getString(mimeColumn) ?: "application/octet-stream",
                            cursor.getLong(createdColumn) * 1000L,
                            modifiedMs,
                            metadata,
                            true,
                        )
                        queued++
                        bytes += output.length()
                    }
                    if (needsThumbnail) {
                        createThumbnail(output, mediaKind, sourceDir)?.let { thumbnail ->
                            enqueue(
                                handle,
                                sourceId,
                                thumbnailId,
                                mediaKind,
                                "thumbnail",
                                thumbnail,
                                "${cursor.getString(nameColumn) ?: "media"}.thumbnail.jpg",
                                "image/jpeg",
                                cursor.getLong(createdColumn) * 1000L,
                                modifiedMs,
                                JSONObject().put("thumbnail_of", sourceId),
                                true,
                            )
                            queued++
                            bytes += thumbnail.length()
                        }
                        if (!needsPrimary) output.delete()
                    }
                } catch (error: Exception) {
                    output.delete()
                    skipped++
                }
            }
        }
        } catch (error: SecurityException) {
            skipped++
        }
        return CollectionResult(queued, bytes, skipped, albums)
    }

    private fun isCameraMedia(bucketName: String?, relativePath: String?): Boolean {
        if (relativePath?.replace('\\', '/')?.startsWith("DCIM/", ignoreCase = true) == true) return true
        return bucketName.equals("Camera", ignoreCase = true)
    }

    private fun enqueue(
        handle: Long,
        sourceAssetId: String,
        sourceResourceId: String,
        mediaKind: String,
        role: String,
        file: File,
        filename: String,
        mimeType: String,
        createdMs: Long,
        modifiedMs: Long,
        metadata: JSONObject,
        removeSource: Boolean,
    ) {
        val input = MobileContractV02.putIdentity(JSONObject())
            .put("source_asset_id", sourceAssetId)
            .put("source_resource_id", sourceResourceId)
            .put("media_kind", mediaKind)
            .put("role", role)
            .put("file_path", file.absolutePath)
            .put("filename", filename)
            .put("mime_type", mimeType)
            .put("source_created_at_ms", createdMs)
            .put("modified_ms", modifiedMs)
            .put("source_size", file.length())
            .put("metadata_json", metadata.toString())
            .put("remove_source_after_prepare", removeSource)
        MobileContractV02.requireEnvelope(NativeBridgeV02.enqueueV02R1(handle, input.toString()))
    }

    private fun createThumbnail(source: File, mediaKind: String, sourceDir: File): File? {
        val bitmap = if (mediaKind == "video") {
            ThumbnailUtils.createVideoThumbnail(source.absolutePath, MediaStore.Video.Thumbnails.MINI_KIND)
        } else {
            val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
            BitmapFactory.decodeFile(source.absolutePath, bounds)
            var sample = 1
            while (bounds.outWidth / sample > 1024 || bounds.outHeight / sample > 1024) sample *= 2
            BitmapFactory.decodeFile(source.absolutePath, BitmapFactory.Options().apply { inSampleSize = sample })
        } ?: return null
        val scale = minOf(1f, 512f / maxOf(bitmap.width, bitmap.height).toFloat())
        val scaled = if (scale < 1f) {
            Bitmap.createScaledBitmap(bitmap, (bitmap.width * scale).toInt(), (bitmap.height * scale).toInt(), true)
        } else {
            bitmap
        }
        val output = File(sourceDir, "${UUID.randomUUID()}.thumbnail.jpg")
        output.outputStream().use { scaled.compress(Bitmap.CompressFormat.JPEG, 82, it) }
        if (scaled !== bitmap) scaled.recycle()
        bitmap.recycle()
        return output
    }

    private fun mergeAlbums(target: MutableMap<String, MutableAlbum>, source: Map<String, MutableAlbum>) {
        for ((id, album) in source) {
            target.getOrPut(id) { MutableAlbum(album.name) }.sourceAssetIds += album.sourceAssetIds
        }
    }

    private data class MutableAlbum(val name: String, val sourceAssetIds: MutableSet<String> = mutableSetOf())
    private data class CollectionResult(
        val discovered: Int,
        val bytes: Long,
        val skipped: Int,
        val albums: Map<String, MutableAlbum>,
    )
}
