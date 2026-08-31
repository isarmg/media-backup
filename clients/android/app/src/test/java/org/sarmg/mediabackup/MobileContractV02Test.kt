package org.sarmg.mediabackup

import java.io.File
import java.nio.file.Files
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class MobileContractV02Test {
    @Test
    fun currentEpochUsesDeclaredSandboxPaths() {
        val sandbox = Files.createTempDirectory("media-mobile-v02-sandbox-").toFile()
        try {
            val currentDatabase = File(sandbox, MobileContractV02.DATABASE_FILENAME)
            val currentStaging = File(sandbox, MobileContractV02.STAGING_DIRECTORY)
            currentDatabase.writeText("current sqlite bytes")
            assertTrue(currentStaging.mkdir())

            assertEquals("agent-v0.2-r1.sqlite", currentDatabase.name)
            assertEquals("backup-staging-v0.2-r1", currentStaging.name)
            assertEquals(sandbox.canonicalFile, currentDatabase.parentFile.canonicalFile)
            assertEquals(sandbox.canonicalFile, currentStaging.parentFile.canonicalFile)
            assertTrue(currentDatabase.isFile)
            assertTrue(currentStaging.isDirectory)
        } finally {
            sandbox.deleteRecursively()
        }
    }
}
