package com.example.photobackup

import android.content.Context
import android.util.Base64
import androidx.security.crypto.EncryptedSharedPreferences
import androidx.security.crypto.MasterKey
import java.security.SecureRandom

data class BackupSnapshot(
    val state: String = "idle",
    val message: String = "尚未开始备份",
    val lastRunAt: Long = 0L,
    val lastSuccessAt: Long = 0L,
    val discovered: Int = 0,
    val completed: Int = 0,
    val failed: Int = 0,
    val uploadedBytes: Long = 0L,
    val currentItem: String = "",
)

class SecureConfig(context: Context) {
    private val masterKey = MasterKey.Builder(context)
        .setKeyScheme(MasterKey.KeyScheme.AES256_GCM)
        .build()
    private val preferences = EncryptedSharedPreferences.create(
        context,
        "photo_backup_secure",
        masterKey,
        EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
        EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM,
    )

    var serverUrl: String
        get() = preferences.getString("server_url", "") ?: ""
        set(value) {
            val normalized = value.trim().trimEnd('/')
            val editor = preferences.edit().putString("server_url", normalized)
            if (normalized != serverUrl) editor.remove("bearer_token")
            editor.apply()
        }

    var username: String
        get() = preferences.getString("username", "") ?: ""
        set(value) {
            val normalized = value.trim()
            val editor = preferences.edit().putString("username", normalized)
            if (normalized != username) editor.remove("bearer_token")
            editor.apply()
        }

    var password: String
        get() = preferences.getString("password", "") ?: ""
        set(value) {
            val editor = preferences.edit().putString("password", value)
            if (value != password) editor.remove("bearer_token")
            editor.apply()
        }

    var bearerToken: String
        get() = preferences.getString("bearer_token", "") ?: ""
        set(value) = preferences.edit().putString("bearer_token", value).apply()

    var autoBackup: Boolean
        get() = preferences.getBoolean("auto_backup", false)
        set(value) = preferences.edit().putBoolean("auto_backup", value).apply()

    var wifiOnly: Boolean
        get() = preferences.getBoolean("wifi_only", true)
        set(value) = preferences.edit().putBoolean("wifi_only", value).apply()

    var chargingOnly: Boolean
        get() = preferences.getBoolean("charging_only", false)
        set(value) = preferences.edit().putBoolean("charging_only", value).apply()

    var backupPhotos: Boolean
        get() = preferences.getBoolean("backup_photos", true)
        set(value) = preferences.edit().putBoolean("backup_photos", value).apply()

    var backupVideos: Boolean
        get() = preferences.getBoolean("backup_videos", true)
        set(value) = preferences.edit().putBoolean("backup_videos", value).apply()

    var cameraOnly: Boolean
        get() = preferences.getBoolean("camera_only", true)
        set(value) = preferences.edit().putBoolean("camera_only", value).apply()

    val masterKeyBase64: String get() = getOrCreateKey("account_master_key")
    val dedupeKeyBase64: String get() = getOrCreateKey("account_dedupe_key")

    fun snapshot(): BackupSnapshot = BackupSnapshot(
        state = preferences.getString("backup_state", "idle") ?: "idle",
        message = preferences.getString("backup_message", "尚未开始备份") ?: "尚未开始备份",
        lastRunAt = preferences.getLong("last_run_at", 0L),
        lastSuccessAt = preferences.getLong("last_success_at", 0L),
        discovered = preferences.getInt("last_discovered", 0),
        completed = preferences.getInt("last_completed", 0),
        failed = preferences.getInt("last_failed", 0),
        uploadedBytes = preferences.getLong("last_uploaded_bytes", 0L),
        currentItem = preferences.getString("current_item", "") ?: "",
    )

    fun saveSnapshot(snapshot: BackupSnapshot) {
        preferences.edit()
            .putString("backup_state", snapshot.state)
            .putString("backup_message", snapshot.message)
            .putLong("last_run_at", snapshot.lastRunAt)
            .putLong("last_success_at", snapshot.lastSuccessAt)
            .putInt("last_discovered", snapshot.discovered)
            .putInt("last_completed", snapshot.completed)
            .putInt("last_failed", snapshot.failed)
            .putLong("last_uploaded_bytes", snapshot.uploadedBytes)
            .putString("current_item", snapshot.currentItem)
            .apply()
    }

    private fun getOrCreateKey(name: String): String {
        preferences.getString(name, null)?.let { return it }
        val bytes = ByteArray(32).also { SecureRandom().nextBytes(it) }
        val encoded = Base64.encodeToString(bytes, Base64.NO_WRAP)
        preferences.edit().putString(name, encoded).commit()
        return encoded
    }
}
