package org.sarmg.mediabackup

import org.json.JSONObject

object MobileContractV02 {
    const val PRODUCT = "media-backup"
    const val APPLICATION_VERSION = "0.2.1"
    const val REVISION = 1
    const val STATE_EPOCH = "media-backup-mobile-v0.2-r2"

    const val DATABASE_FILENAME = "agent-v0.2-r2.sqlite"
    const val STAGING_DIRECTORY = "backup-staging-v0.2-r2"
    const val PREFERENCES_FILE = "media_backup_secure_v0_2_r2"
    const val TOKEN_KEY = "bearer_token_v0_2_r2"
    const val WORK_TAG = "media-backup-v0.2-r2"
    const val NOW_WORK = "media-backup-now-v0.2-r2"
    const val PERIODIC_WORK = "media-backup-periodic-v0.2-r2"
    const val NOTIFICATION_CHANNEL = "media-backup-v0.2-r2"

    fun putIdentity(target: JSONObject): JSONObject = target
        .put("product", PRODUCT)
        .put("application_version", APPLICATION_VERSION)
        .put("revision", REVISION)
        .put("state_epoch", STATE_EPOCH)

    fun requireEnvelope(raw: String): JSONObject {
        val envelope = JSONObject(raw)
        val allowed = setOf(
            "product",
            "application_version",
            "revision",
            "state_epoch",
            "ok",
            "value",
            "error",
        )
        val keys = mutableSetOf<String>()
        val iterator = envelope.keys()
        while (iterator.hasNext()) keys += iterator.next()
        require(keys == allowed) { "Rust Agent returned an unknown v0.2 envelope shape" }
        require(envelope.getString("product") == PRODUCT) { "Rust Agent product mismatch" }
        require(envelope.getString("application_version") == APPLICATION_VERSION) {
            "Rust Agent application version mismatch"
        }
        require(envelope.getInt("revision") == REVISION) { "Rust Agent revision mismatch" }
        require(envelope.getString("state_epoch") == STATE_EPOCH) { "Rust Agent state epoch mismatch" }
        if (!envelope.getBoolean("ok")) {
            error(envelope.optString("error", "Rust Agent v0.2 operation failed"))
        }
        return envelope
    }

    fun requireIdentity(value: JSONObject) {
        require(value.getString("product") == PRODUCT) { "Rust Agent job product mismatch" }
        require(value.getString("application_version") == APPLICATION_VERSION) {
            "Rust Agent job application version mismatch"
        }
        require(value.getInt("revision") == REVISION) { "Rust Agent job revision mismatch" }
        require(value.getString("state_epoch") == STATE_EPOCH) { "Rust Agent job state epoch mismatch" }
    }
}
