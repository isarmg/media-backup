import Foundation
import MediaBackupRust

private func withUTF8<T>(_ text: String, _ body: (UnsafePointer<UInt8>?, Int) throws -> T) rethrows -> T {
    try Array(text.utf8).withUnsafeBufferPointer { try body($0.baseAddress, $0.count) }
}

private func nativeCall(_ operation: (UnsafeMutablePointer<SarmgFfiResultV2>) -> Int32) throws -> (value: UInt64, data: Data) {
    var result = SarmgFfiResultV2()
    let status = operation(&result)
    guard result.abi_revision == UInt32(SARMG_FFI_ABI_REVISION) else {
        throw AgentFailure.message("Media Backup native result ABI mismatch")
    }
    defer { _ = sarmg_ffi_result_free_v2(&result) }
    guard status == result.status, result.bytes.length <= Int(SARMG_FFI_MAX_OUTPUT_BYTES),
          result.bytes.data != nil || result.bytes.length == 0 else {
        throw AgentFailure.message("Media Backup native result is invalid")
    }
    let data = result.bytes.data.map { Data(bytes: $0, count: result.bytes.length) } ?? Data()
    guard status == Int32(SARMG_FFI_OK) else {
        throw AgentFailure.message(String(data: data, encoding: .utf8) ?? "Native operation failed")
    }
    return (result.value, data)
}

struct UploadPart: Codable {
    let index: UInt32
    let size: UInt64
    let blake3: String
}

enum JSONValue: Codable {
    case string(String), number(Double), bool(Bool), object([String: JSONValue]), array([JSONValue]), null

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() { self = .null }
        else if let value = try? container.decode(Bool.self) { self = .bool(value) }
        else if let value = try? container.decode(Double.self) { self = .number(value) }
        else if let value = try? container.decode(String.self) { self = .string(value) }
        else if let value = try? container.decode([String: JSONValue].self) { self = .object(value) }
        else { self = .array(try container.decode([JSONValue].self)) }
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .string(let value): try container.encode(value)
        case .number(let value): try container.encode(value)
        case .bool(let value): try container.encode(value)
        case .object(let value): try container.encode(value)
        case .array(let value): try container.encode(value)
        case .null: try container.encodeNil()
        }
    }
}

struct CreateUpload: Codable {
    let sourceAssetId: String
    let sourceResourceId: String
    let mediaKind: String
    let role: String
    let filename: String
    let mimeType: String
    let sourceCreatedAtMs: Int64
    let storageEncoding: StorageEncoding
    let contentSize: UInt64
    let contentBlake3: String
    let metadata: JSONValue?
    let parts: [UploadPart]
}

struct LocalPart: Codable {
    let index: UInt32
    let path: String
}

struct PreparedJob: Codable {
    let product: String
    let applicationVersion: String
    let revision: UInt32
    let stateEpoch: String
    let jobId: String
    let request: CreateUpload
    let localParts: [LocalPart]
}

struct EnqueueInput: Codable {
    let product: String
    let applicationVersion: String
    let revision: UInt32
    let stateEpoch: String
    let sourceAssetId: String
    let sourceResourceId: String
    let mediaKind: String
    let role: String
    let filePath: String
    let filename: String
    let mimeType: String
    let sourceCreatedAtMs: Int64
    let modifiedMs: Int64
    let sourceSize: UInt64
    let metadataJson: String?
    let removeSourceAfterPrepare: Bool
}

private struct Envelope<T: Decodable>: Decodable {
    let product: String
    let applicationVersion: String
    let revision: UInt32
    let stateEpoch: String
    let ok: Bool
    let value: T?
    let error: String?
}

private struct EmptyValue: Decodable {}

final class RustAgent {
    private let handle: UInt64
    private let encoder: JSONEncoder = {
        let value = JSONEncoder()
        value.keyEncodingStrategy = .convertToSnakeCase
        return value
    }()
    private let decoder: JSONDecoder = {
        let value = JSONDecoder()
        value.keyDecodingStrategy = .convertFromSnakeCase
        return value
    }()

    init(databasePath: String) throws {
        guard mb_ffi_abi_revision() == UInt32(SARMG_FFI_ABI_REVISION) else {
            throw AgentFailure.message("Media Backup native ABI mismatch")
        }
        let config: [String: Any] = [
            "product": MobileContractV02.product,
            "application_version": MobileContractV02.applicationVersion,
            "revision": MobileContractV02.revision,
            "state_epoch": MobileContractV02.stateEpoch,
            "part_size": 16 * 1024 * 1024,
        ]
        let data = try JSONSerialization.data(withJSONObject: config)
        let opened = try nativeCall { output in
            withUTF8(databasePath) { path, pathLength in
                data.withUnsafeBytes { bytes in
                    mb_open_v2(path, pathLength, bytes.bindMemory(to: UInt8.self).baseAddress, bytes.count, output)
                }
            }
        }
        guard opened.value != 0 else { throw AgentFailure.message("Native open returned an invalid handle") }
        handle = opened.value
    }

    deinit { _ = try? nativeCall { mb_close_v2(handle, $0) } }

    func needs(asset: String, resource: String, modifiedMs: Int64) throws -> Bool {
        let result = try nativeCall { output in
            withUTF8(asset) { assetPointer, assetLength in
                withUTF8(resource) { resourcePointer, resourceLength in
                    mb_needs_v2(handle, assetPointer, assetLength, resourcePointer, resourceLength, modifiedMs, output)
                }
            }
        }
        guard result.value <= 1 else { throw AgentFailure.message("Native needs returned an invalid boolean") }
        return result.value == 1
    }

    func enqueue(_ input: EnqueueInput) throws {
        let data = try encoder.encode(input)
        let _: String? = try call { output in
            data.withUnsafeBytes { bytes in mb_enqueue_v2(handle, bytes.bindMemory(to: UInt8.self).baseAddress, bytes.count, output) }
        }
    }

    func next(stagingRoot: String) throws -> PreparedJob? {
        let job: PreparedJob? = try call { output in
            withUTF8(stagingRoot) { mb_next_v2(handle, $0, $1, output) }
        }
        if let job {
            try MobileContractV02.requireIdentity(product: job.product, applicationVersion: job.applicationVersion,
                                                 revision: job.revision, stateEpoch: job.stateEpoch)
        }
        return job
    }

    func markUpload(job: String, upload: String) throws {
        let _: EmptyValue? = try call { output in
            withUTF8(job) { jobPointer, jobLength in
                withUTF8(upload) { mb_mark_upload_v2(handle, jobPointer, jobLength, $0, $1, output) }
            }
        }
    }
    func markPart(job: String, index: UInt32) throws {
        let _: EmptyValue? = try call { output in withUTF8(job) { mb_mark_part_v2(handle, $0, $1, index, output) } }
    }
    func markComplete(job: String) throws {
        let _: EmptyValue? = try call { output in withUTF8(job) { mb_mark_complete_v2(handle, $0, $1, output) } }
    }
    func markFailed(job: String, error: String) {
        let _: EmptyValue? = try? call { output in
            withUTF8(job) { jobPointer, jobLength in
                withUTF8(error) { mb_mark_failed_v2(handle, jobPointer, jobLength, $0, $1, 1, output) }
            }
        }
    }
    func statsDescription() -> String {
        let stats: JSONValue? = try? call { mb_stats_v2(handle, $0) }
        return stats.map { String(describing: $0) } ?? ""
    }

    private func call<T: Decodable>(_ operation: (UnsafeMutablePointer<SarmgFfiResultV2>) -> Int32) throws -> T? {
        let data = try nativeCall(operation).data
        let raw = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        let expectedKeys = Set(["product", "application_version", "revision", "state_epoch", "ok", "value", "error"])
        guard raw.map({ Set($0.keys) }) == expectedKeys else {
            throw AgentFailure.message("Rust Agent returned an invalid current envelope")
        }
        let envelope = try decoder.decode(Envelope<T>.self, from: data)
        try MobileContractV02.requireIdentity(product: envelope.product, applicationVersion: envelope.applicationVersion,
                                             revision: envelope.revision, stateEpoch: envelope.stateEpoch)
        guard envelope.ok && envelope.error == nil else {
            throw AgentFailure.message("Rust Agent returned an invalid success envelope")
        }
        return envelope.value
    }
}

enum AgentFailure: Error { case message(String) }
