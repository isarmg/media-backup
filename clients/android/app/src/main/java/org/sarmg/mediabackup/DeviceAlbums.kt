package org.sarmg.mediabackup

import android.content.Context
import android.provider.MediaStore

data class DeviceAlbum(val id: String, val name: String, val count: Int)

object DeviceAlbums {
    fun list(context: Context): List<DeviceAlbum> {
        val albums = linkedMapOf<String, Pair<String, Int>>()
        for (collection in listOf(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, MediaStore.Video.Media.EXTERNAL_CONTENT_URI)) {
            context.contentResolver.query(
                collection,
                arrayOf(MediaStore.Images.ImageColumns.BUCKET_ID, MediaStore.Images.ImageColumns.BUCKET_DISPLAY_NAME),
                null,
                null,
                null,
            )?.use { cursor ->
                val idColumn = cursor.getColumnIndex(MediaStore.Images.ImageColumns.BUCKET_ID)
                val nameColumn = cursor.getColumnIndex(MediaStore.Images.ImageColumns.BUCKET_DISPLAY_NAME)
                while (cursor.moveToNext()) {
                    if (idColumn < 0) continue
                    val id = cursor.getString(idColumn) ?: continue
                    val name = if (nameColumn >= 0) cursor.getString(nameColumn) ?: "未命名相册" else "未命名相册"
                    val current = albums[id]
                    albums[id] = name to ((current?.second ?: 0) + 1)
                }
            }
        }
        return albums.map { DeviceAlbum(it.key, it.value.first, it.value.second) }
            .sortedWith(compareByDescending<DeviceAlbum> { it.count }.thenBy { it.name.lowercase() })
    }
}
