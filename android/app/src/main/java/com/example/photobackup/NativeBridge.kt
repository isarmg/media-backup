package com.example.photobackup

object NativeBridge {
    init {
        System.loadLibrary("photo_backup_mobile")
    }

    external fun nativeOpen(databasePath: String, configJson: String): Long
    external fun nativeClose(handle: Long)
    external fun nativeNeeds(handle: Long, sourceAssetId: String, sourceResourceId: String, modifiedMs: Long): Boolean
    external fun nativeEnqueue(handle: Long, inputJson: String): String
    external fun nativeNext(handle: Long, stagingRoot: String): String
    external fun nativeMarkUpload(handle: Long, jobId: String, uploadId: String): String
    external fun nativeMarkPart(handle: Long, jobId: String, index: Int): String
    external fun nativeMarkComplete(handle: Long, jobId: String): String
    external fun nativeMarkFailed(handle: Long, jobId: String, message: String, retryable: Boolean): String
    external fun nativeStats(handle: Long): String
}
