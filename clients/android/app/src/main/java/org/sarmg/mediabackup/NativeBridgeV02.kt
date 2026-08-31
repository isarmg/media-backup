package org.sarmg.mediabackup

object NativeBridgeV02 {
    init {
        System.loadLibrary("media_backup_mobile")
    }

    external fun openV02R1(databasePath: String, configJson: String): Long
    external fun closeV02R1(handle: Long)
    external fun needsV02R1(
        handle: Long,
        sourceAssetId: String,
        sourceResourceId: String,
        modifiedMs: Long,
    ): Boolean
    external fun enqueueV02R1(handle: Long, inputJson: String): String
    external fun nextV02R1(handle: Long, stagingRoot: String): String
    external fun markUploadV02R1(handle: Long, jobId: String, uploadId: String): String
    external fun markPartV02R1(handle: Long, jobId: String, index: Int): String
    external fun markCompleteV02R1(handle: Long, jobId: String): String
    external fun markFailedV02R1(
        handle: Long,
        jobId: String,
        message: String,
        retryable: Boolean,
    ): String
    external fun statsV02R1(handle: Long): String
}
