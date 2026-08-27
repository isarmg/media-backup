package com.example.photobackup

import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.asRequestBody
import okhttp3.RequestBody.Companion.toRequestBody
import org.json.JSONObject
import java.io.File
import java.util.concurrent.TimeUnit

class BackupApi(private val serverUrl: String, private var bearerToken: String) {
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
        val request = Request.Builder().url("$serverUrl/v1/auth/bootstrap").post(body).build()
        val response = client.newCall(request).execute()
        response.use {
            val text = it.body.string()
            if (!it.isSuccessful) error("设备注册失败: ${it.code} $text")
            bearerToken = JSONObject(text).getString("bearer_token")
            return bearerToken
        }
    }

    fun createUpload(requestJson: String): JSONObject = jsonRequest("$serverUrl/v1/uploads", "POST", requestJson)

    fun uploadPart(uploadId: String, index: Int, file: File) {
        val request = authenticated(Request.Builder().url("$serverUrl/v1/uploads/$uploadId/parts/$index"))
            .put(file.asRequestBody(binaryType))
            .build()
        client.newCall(request).execute().use {
            if (!it.isSuccessful) error("分块上传失败: ${it.code} ${it.body.string()}")
        }
    }

    fun complete(uploadId: String): JSONObject =
        jsonRequest("$serverUrl/v1/uploads/$uploadId/complete", "POST", "{}")

    private fun jsonRequest(url: String, method: String, bodyText: String): JSONObject {
        val builder = authenticated(Request.Builder().url(url))
        val body = bodyText.toRequestBody(jsonType)
        when (method) {
            "POST" -> builder.post(body)
            else -> error("unsupported method")
        }
        client.newCall(builder.build()).execute().use {
            val text = it.body.string()
            if (!it.isSuccessful) error("服务端请求失败: ${it.code} $text")
            return JSONObject(text)
        }
    }

    private fun authenticated(builder: Request.Builder): Request.Builder =
        builder.header("Authorization", "Bearer $bearerToken")
}
