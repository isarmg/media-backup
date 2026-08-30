import XCTest
@testable import PhotoBackup

final class MobileContractV02Tests: XCTestCase {
    func testV02SandboxLeavesV01DatabaseAndStagingUntouched() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("photo-mobile-v02-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: false)
        defer { try? FileManager.default.removeItem(at: root) }

        let legacyDatabase = root.appendingPathComponent("agent.sqlite")
        let legacyStaging = root.appendingPathComponent("backup-staging", isDirectory: true)
        let legacyMarker = legacyStaging.appendingPathComponent("v01-marker")
        let databaseBytes = Data("v0.1 sqlite bytes".utf8)
        let stagingBytes = Data("v0.1 staging bytes".utf8)
        try databaseBytes.write(to: legacyDatabase)
        try FileManager.default.createDirectory(at: legacyStaging, withIntermediateDirectories: false)
        try stagingBytes.write(to: legacyMarker)

        let currentDatabase = root.appendingPathComponent(MobileContractV02.databaseFilename)
        let currentStaging = root.appendingPathComponent(MobileContractV02.stagingDirectory, isDirectory: true)
        try Data("v0.2 sqlite bytes".utf8).write(to: currentDatabase)
        try FileManager.default.createDirectory(at: currentStaging, withIntermediateDirectories: false)

        XCTAssertNotEqual(currentDatabase.standardizedFileURL, legacyDatabase.standardizedFileURL)
        XCTAssertNotEqual(currentStaging.standardizedFileURL, legacyStaging.standardizedFileURL)
        XCTAssertEqual(try Data(contentsOf: legacyDatabase), databaseBytes)
        XCTAssertEqual(try Data(contentsOf: legacyMarker), stagingBytes)
    }

    func testCredentialAndPreferenceDomainsAreV02Only() {
        XCTAssertEqual(MobileContractV02.product, "photo-backup")
        XCTAssertEqual(MobileContractV02.applicationVersion, "0.2.0")
        XCTAssertEqual(MobileContractV02.revision, 1)
        XCTAssertTrue(MobileContractV02.keychainService.contains(".v02."))
        XCTAssertTrue(MobileContractV02.tokenKey.contains("v0_2_r1"))
    }

    func testV02PreferencesDoNotReadOrDeleteV01TokenState() {
        let legacySuiteName = "com.example.photobackup"
        let legacy = UserDefaults(suiteName: legacySuiteName)!
        let legacyKey = "bearer_token"
        legacy.set("v0.1 token", forKey: legacyKey)
        MobileContractV02.preferences.removeObject(forKey: MobileContractV02.tokenKey)
        defer {
            legacy.removeObject(forKey: legacyKey)
            MobileContractV02.preferences.removeObject(forKey: MobileContractV02.tokenKey)
        }

        XCTAssertNil(MobileContractV02.preferences.string(forKey: MobileContractV02.tokenKey))
        XCTAssertEqual(legacy.string(forKey: legacyKey), "v0.1 token")
    }
}
