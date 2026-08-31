package com.example.mediabackup.v02

import java.io.File
import java.nio.file.Files
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class MobileContractV02Test {
    @Test
    fun currentEpochUsesAnIsolatedSandboxAndLeavesV01StateUntouched() {
        val sandbox = Files.createTempDirectory("photo-mobile-v02-sandbox-").toFile()
        try {
            val legacyDatabase = File(sandbox, "agent.sqlite")
            val legacyStaging = File(sandbox, "backup-staging").also { it.mkdirs() }
            val legacyMarker = File(legacyStaging, "v01-marker")
            val databaseBytes = "v0.1 sqlite bytes".toByteArray()
            val stagingBytes = "v0.1 staging bytes".toByteArray()
            legacyDatabase.writeBytes(databaseBytes)
            legacyMarker.writeBytes(stagingBytes)

            val currentDatabase = File(sandbox, MobileContractV02.DATABASE_FILENAME)
            val currentStaging = File(sandbox, MobileContractV02.STAGING_DIRECTORY)
            currentDatabase.writeText("v0.2 sqlite bytes")
            assertTrue(currentStaging.mkdir())

            assertFalse(currentDatabase.canonicalPath == legacyDatabase.canonicalPath)
            assertFalse(currentStaging.canonicalPath == legacyStaging.canonicalPath)
            assertArrayEquals(databaseBytes, legacyDatabase.readBytes())
            assertArrayEquals(stagingBytes, legacyMarker.readBytes())
        } finally {
            sandbox.deleteRecursively()
        }
    }
}
