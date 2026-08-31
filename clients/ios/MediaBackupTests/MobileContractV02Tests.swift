import XCTest
@testable import MediaBackup

final class MobileContractV02Tests: XCTestCase {
    func testCurrentContractUsesDeclaredSandboxPaths() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("media-mobile-v02-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: false)
        defer { try? FileManager.default.removeItem(at: root) }

        let currentDatabase = root.appendingPathComponent(MobileContractV02.databaseFilename)
        let currentStaging = root.appendingPathComponent(MobileContractV02.stagingDirectory, isDirectory: true)
        try Data("current sqlite bytes".utf8).write(to: currentDatabase)
        try FileManager.default.createDirectory(at: currentStaging, withIntermediateDirectories: false)

        XCTAssertEqual(currentDatabase.lastPathComponent, "agent-v0.2-r1.sqlite3")
        XCTAssertEqual(currentStaging.lastPathComponent, "backup-staging-v0.2-r1")
        XCTAssertEqual(currentDatabase.deletingLastPathComponent().standardizedFileURL, root.standardizedFileURL)
        XCTAssertEqual(currentStaging.deletingLastPathComponent().standardizedFileURL, root.standardizedFileURL)
        XCTAssertTrue(FileManager.default.fileExists(atPath: currentDatabase.path))
        XCTAssertTrue(FileManager.default.fileExists(atPath: currentStaging.path))
    }

    func testCredentialAndPreferenceDomainsAreV02Only() {
        XCTAssertEqual(MobileContractV02.product, "media-backup")
        XCTAssertEqual(MobileContractV02.applicationVersion, "0.2.0")
        XCTAssertEqual(MobileContractV02.revision, 1)
        XCTAssertTrue(MobileContractV02.keychainService.contains(".v02."))
        XCTAssertTrue(MobileContractV02.tokenKey.contains("v0_2_r1"))
    }

    func testCurrentPreferenceDomainRoundTripsCurrentToken() {
        MobileContractV02.preferences.removeObject(forKey: MobileContractV02.tokenKey)
        defer {
            MobileContractV02.preferences.removeObject(forKey: MobileContractV02.tokenKey)
        }

        MobileContractV02.preferences.set("current-token", forKey: MobileContractV02.tokenKey)
        XCTAssertEqual(
            MobileContractV02.preferences.string(forKey: MobileContractV02.tokenKey),
            "current-token"
        )
    }
}
