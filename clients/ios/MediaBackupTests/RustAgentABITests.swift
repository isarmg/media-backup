import XCTest
import MediaBackupRust
@testable import MediaBackup

final class RustAgentABITests: XCTestCase {
    func testImportedHeaderMatchesNativeLibraryAndErrorsAreExplicit() {
        XCTAssertEqual(mb_ffi_abi_revision(), UInt32(SARMG_FFI_ABI_REVISION))
        var output = SarmgFfiResultV2()
        XCTAssertEqual(mb_stats_v2(0, &output), Int32(SARMG_FFI_INVALID_HANDLE))
        XCTAssertEqual(output.status, Int32(SARMG_FFI_INVALID_HANDLE))
        XCTAssertEqual(output.value, 0)
        XCTAssertGreaterThan(output.bytes.length, 0)
        XCTAssertEqual(sarmg_ffi_result_free_v2(&output), Int32(SARMG_FFI_OK))
        XCTAssertNil(output.bytes.data)
        XCTAssertEqual(output.bytes.length, 0)
        XCTAssertEqual(sarmg_ffi_result_free_v2(&output), Int32(SARMG_FFI_OK))
    }

    func testRustAgentUsesLengthDelimitedUnicodeAndClosesBeforeReopen() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("ffi-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: false)
        defer { try? FileManager.default.removeItem(at: root) }
        let path = root.appendingPathComponent(MobileContractV02.databaseFilename).path
        var agent: RustAgent? = try RustAgent(databasePath: path)
        XCTAssertTrue(try XCTUnwrap(agent).needs(asset: "中文😀", resource: "resource", modifiedMs: 1))
        agent = nil
        let reopened = try RustAgent(databasePath: path)
        XCTAssertTrue(try reopened.needs(asset: "中文😀", resource: "resource", modifiedMs: 1))
        XCTAssertThrowsError(try RustAgent(databasePath: root.appendingPathComponent("wrong.sqlite").path))
    }
}
