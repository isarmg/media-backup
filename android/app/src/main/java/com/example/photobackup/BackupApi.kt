package com.example.photobackup.v02

import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.asRequestBody
import okhttp3.RequestBody.Companion.toRequestBody
import org.json.JSONObject
import java.io.File
import java.io.OutputStream
import java.util.concurrent.TimeUnit

class BackupApi(private val serverUrl: String, private var bearerToken: String) {
    init {
        require(serverUrl.startsWith("https://")) { "服务器地址必须使用 HTTPS" }
    }
    private val jsonType = "application/json".toMediaType()
    private val binaryType = "application/octet-stream".toMediaType()
    private val client = OkHttpClient.Builder()
        .connectTimeout(30, TimeUnit.SECONDS)
        .readTimeout(10, TimeUnit.MINUTES)
        .writeTimeout(10, TimeUnit.MINUTES)
        .build()

    fun health(): Boolean {
        val request = Request.Builder().url("$serverUrl/health").get().build()
        client.newCall(request).execute().use {
            if (!it.isSuccessful) error("服务端健康检查失败: ${it.code}")
            return true
        }
    }

    fun bootstrap(username: String, password: String, deviceName: String): String {
        val body = JSONObject()
            .put("username", username)
            .put("password", password)
            .put("device_name", deviceName)
            .put("platform", "android")
            .toString()
            .toRequestBody(jsonType)
        val request = Request.Builder().url("$serverUrl/v2/auth/bootstrap").post(body).build()
        val response = client.newCall(request).execute()
        response.use {
            val text = it.body.string()
            if (!it.isSuccessful) error("设备注册失败: ${it.code} $text")
            bearerToken = JSONObject(text).getString("bearer_token")
            return bearerToken
        }
    }

    fun createUpload(requestJson: String): JSONObject = jsonRequest("$serverUrl/v2/uploads", "POST", requestJson)

    fun uploadPart(uploadId: String, index: Int, file: File) {
        val request = authenticated(Request.Builder().url("$serverUrl/v2/uploads/$uploadId/parts/$index"))
            .put(file.asRequestBody(binaryType))
            .build()
        client.newCall(request).execute().use {
            if (!it.isSuccessful) error("分块上传失败: ${it.code} ${it.body.string()}")
        }
    }

    fun complete(uploadId: String): JSONObject =
        jsonRequest("$serverUrl/v2/uploads/$uploadId/complete", "POST", "{}")

    fun syncAlbum(
        sourceAlbumId: String,
        name: String,
        sourceAssetIds: Set<String>,
        replaceMembers: Boolean,
    ): JSONObject {
        val assets = org.json.JSONArray()
        sourceAssetIds.forEach(assets::put)
        val body = JSONObject()
            .put("source_album_id", sourceAlbumId)
            .put("name", name)
            .put("source_asset_ids", assets)
            .put("replace_members", replaceMembers)
        return jsonRequest("$serverUrl/v2/albums", "POST", body.toString())
    }

    fun timeline(cursor: String? = null, trashed: Boolean = false): JSONObject {
        val suffix = buildString {
            append("?limit=100&trashed=").append(trashed)
            if (!cursor.isNullOrBlank()) append("&cursor=").append(java.net.URLEncoder.encode(cursor, "UTF-8"))
        }
        return jsonRequest("$serverUrl/v2/timeline$suffix", "GET", null)
    }

    fun sync(after: Long): JSONObject =
        jsonRequest("$serverUrl/v2/sync?after=$after&limit=1000", "GET", null)

    fun updateAsset(assetId: String, favorite: Boolean? = null, archived: Boolean? = null): JSONObject {
        val body = JSONObject()
        favorite?.let { body.put("favorite", it) }
        archived?.let { body.put("archived", it) }
        return jsonRequest("$serverUrl/v2/assets/$assetId", "PATCH", body.toString())
    }

    fun trashAsset(assetId: String) {
        jsonRequest("$serverUrl/v2/assets/$assetId/trash", "POST", "{}", expectJson = false)
    }

    fun restoreAsset(assetId: String) {
        jsonRequest("$serverUrl/v2/assets/$assetId/restore", "POST", "{}", expectJson = false)
    }

    fun duplicateGroups(): org.json.JSONArray =
        jsonArrayRequest("$serverUrl/v2/duplicates?limit=50")

    fun listTags(): org.json.JSONArray = jsonArrayRequest("$serverUrl/v2/tags")

    fun createTag(name: String): JSONObject = jsonRequest(
        "$serverUrl/v2/tags",
        "POST",
        JSONObject().put("name", name).toString(),
    )

    fun addTagAsset(tagId: String, assetId: String) {
        jsonRequest("$serverUrl/v2/tags/$tagId/assets/$assetId", "POST", "{}", expectJson = false)
    }

    fun download(path: String, output: OutputStream) {
        val request = authenticated(Request.Builder().url(resolve(path))).get().build()
        client.newCall(request).execute().use {
            if (!it.isSuccessful) error("下载失败: ${it.code} ${it.body.string()}")
            it.body.byteStream().use { input -> input.copyTo(output) }
        }
    }

    fun downloadBytes(path: String): ByteArray {
        val request = authenticated(Request.Builder().url(resolve(path))).get().build()
        client.newCall(request).execute().use {
            if (!it.isSuccessful) error("下载失败: ${it.code} ${it.body.string()}")
            return it.body.bytes()
        }
    }

    private fun jsonRequest(url: String, method: String, bodyText: String?, expectJson: Boolean = true): JSONObject {
        val builder = authenticated(Request.Builder().url(url))
        when (method) {
            "GET" -> builder.get()
            "POST" -> builder.post((bodyText ?: "{}").toRequestBody(jsonType))
            "PATCH" -> builder.patch((bodyText ?: "{}").toRequestBody(jsonType))
            else -> error("unsupported method")
        }
        client.newCall(builder.build()).execute().use {
            val text = it.body.string()
            if (!it.isSuccessful) error("服务端请求失败: ${it.code} $text")
            return if (expectJson && text.isNotBlank()) JSONObject(text) else JSONObject()
        }
    }

    private fun jsonArrayRequest(url: String): org.json.JSONArray {
        val request = authenticated(Request.Builder().url(url)).get().build()
        client.newCall(request).execute().use {
            val text = it.body.string()
            if (!it.isSuccessful) error("服务端请求失败: ${it.code} $text")
            return org.json.JSONArray(text)
        }
    }

    private fun resolve(path: String): String = if (path.startsWith("http://") || path.startsWith("https://")) path else "$serverUrl$path"

    private fun authenticated(builder: Request.Builder): Request.Builder =
        builder.header("Authorization", "Bearer $bearerToken")
}
