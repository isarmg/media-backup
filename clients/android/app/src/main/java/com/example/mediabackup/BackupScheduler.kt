package com.example.mediabackup.v02

import android.content.Context
import androidx.work.BackoffPolicy
import androidx.work.Constraints
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.workDataOf
import java.util.UUID
import java.util.concurrent.TimeUnit

object BackupScheduler {
    const val TAG = MobileContractV02.WORK_TAG
    const val SOURCE_KEY = "backup_source"
    const val SOURCE_MANUAL = "manual"
    const val SOURCE_AUTOMATIC = "automatic"

    private const val NOW_WORK = MobileContractV02.NOW_WORK
    private const val PERIODIC_WORK = MobileContractV02.PERIODIC_WORK
    private const val PERIOD_HOURS = 6L

    fun syncAutomatic(context: Context, config: SecureConfig) {
        val manager = WorkManager.getInstance(context)
        if (!config.autoBackup || config.serverUrl.isBlank() || config.username.isBlank() || config.password.isBlank()) {
            manager.cancelUniqueWork(PERIODIC_WORK)
            return
        }
        manager.enqueueUniquePeriodicWork(
            PERIODIC_WORK,
            ExistingPeriodicWorkPolicy.UPDATE,
            periodicRequest(config, 0L),
        )
    }

    fun enqueueNow(context: Context, config: SecureConfig): UUID {
        val request = OneTimeWorkRequestBuilder<BackupWorker>()
            .setConstraints(constraints(config))
            .setInputData(workDataOf(SOURCE_KEY to SOURCE_MANUAL))
            .setBackoffCriteria(BackoffPolicy.EXPONENTIAL, 30L, TimeUnit.SECONDS)
            .addTag(TAG)
            .build()
        WorkManager.getInstance(context)
            .enqueueUniqueWork(NOW_WORK, ExistingWorkPolicy.REPLACE, request)
        return request.id
    }

    fun stopCurrent(context: Context, config: SecureConfig) {
        val manager = WorkManager.getInstance(context)
        manager.cancelAllWorkByTag(TAG)
        if (config.autoBackup) {
            manager.enqueueUniquePeriodicWork(
                PERIODIC_WORK,
                ExistingPeriodicWorkPolicy.CANCEL_AND_REENQUEUE,
                periodicRequest(config, PERIOD_HOURS),
            )
        }
    }

    private fun periodicRequest(config: SecureConfig, initialDelayHours: Long) =
        PeriodicWorkRequestBuilder<BackupWorker>(PERIOD_HOURS, TimeUnit.HOURS)
            .setConstraints(constraints(config))
            .setInputData(workDataOf(SOURCE_KEY to SOURCE_AUTOMATIC))
            .setInitialDelay(initialDelayHours, TimeUnit.HOURS)
            .setBackoffCriteria(BackoffPolicy.EXPONENTIAL, 30L, TimeUnit.SECONDS)
            .addTag(TAG)
            .build()

    private fun constraints(config: SecureConfig): Constraints = Constraints.Builder()
        .setRequiredNetworkType(if (config.wifiOnly) NetworkType.UNMETERED else NetworkType.CONNECTED)
        .setRequiresCharging(config.chargingOnly)
        .setRequiresBatteryNotLow(true)
        .setRequiresStorageNotLow(true)
        .build()
}
