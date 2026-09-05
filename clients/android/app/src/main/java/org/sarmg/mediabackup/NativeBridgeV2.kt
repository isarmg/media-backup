package org.sarmg.mediabackup

object NativeBridgeV2 {
    init {
        System.loadLibrary("media_backup_mobile")
        check(abiRevision() == EXPECTED_ABI_REVISION) {
            "Media Backup native ABI mismatch"
        }
    }

    external fun abiRevision(): Int
    external fun open(databasePath: String, configJson: String): Long
    external fun close(handle: Long)
    external fun needs(
        handle: Long,
        sourceAssetId: String,
        sourceResourceId: String,
        modifiedMs: Long,
    ): Boolean
    external fun enqueue(handle: Long, inputJson: String): String
    external fun next(handle: Long, stagingRoot: String): String
    external fun markUpload(handle: Long, jobId: String, uploadId: String): String
    external fun markPart(handle: Long, jobId: String, index: Int): String
    external fun markComplete(handle: Long, jobId: String): String
    external fun markFailed(
        handle: Long,
        jobId: String,
        message: String,
        retryable: Boolean,
    ): String
    external fun stats(handle: Long): String

    private const val EXPECTED_ABI_REVISION = 2
}
