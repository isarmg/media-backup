import Foundation

@_silgen_name("pb_v0_2_r1_open") private func photoBackupV02R1Open(_ databasePath: UnsafePointer<CChar>, _ configJSON: UnsafePointer<CChar>) -> UInt64
@_silgen_name("pb_v0_2_r1_close") private func photoBackupV02R1Close(_ handle: UInt64)
@_silgen_name("pb_v0_2_r1_needs") private func photoBackupV02R1Needs(_ handle: UInt64, _ asset: UnsafePointer<CChar>, _ resource: UnsafePointer<CChar>, _ modified: Int64) -> Bool
@_silgen_name("pb_v0_2_r1_enqueue") private func photoBackupV02R1Enqueue(_ handle: UInt64, _ input: UnsafePointer<CChar>) -> UnsafeMutablePointer<CChar>?
@_silgen_name("pb_v0_2_r1_next") private func photoBackupV02R1Next(_ handle: UInt64, _ staging: UnsafePointer<CChar>) -> UnsafeMutablePointer<CChar>?
@_silgen_name("pb_v0_2_r1_mark_upload") private func photoBackupV02R1MarkUpload(_ handle: UInt64, _ job: UnsafePointer<CChar>, _ upload: UnsafePointer<CChar>) -> UnsafeMutablePointer<CChar>?
@_silgen_name("pb_v0_2_r1_mark_part") private func photoBackupV02R1MarkPart(_ handle: UInt64, _ job: UnsafePointer<CChar>, _ index: UInt32) -> UnsafeMutablePointer<CChar>?
@_silgen_name("pb_v0_2_r1_mark_complete") private func photoBackupV02R1MarkComplete(_ handle: UInt64, _ job: UnsafePointer<CChar>) -> UnsafeMutablePointer<CChar>?
@_silgen_name("pb_v0_2_r1_mark_failed") private func photoBackupV02R1MarkFailed(_ handle: UInt64, _ job: UnsafePointer<CChar>, _ error: UnsafePointer<CChar>, _ retryable: Bool) -> UnsafeMutablePointer<CChar>?
@_silgen_name("pb_v0_2_r1_stats") private func photoBackupV02R1Stats(_ handle: UInt64) -> UnsafeMutablePointer<CChar>?
@_silgen_name("pb_v0_2_r1_last_error") private func photoBackupV02R1LastError() -> UnsafePointer<CChar>?
@_silgen_name("pb_v0_2_r1_string_free") private func photoBackupV02R1StringFree(_ value: UnsafeMutablePointer<CChar>?)

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
        let config: [String: Any] = [
            "product": MobileContractV02.product,
            "application_version": MobileContractV02.applicationVersion,
            "revision": MobileContractV02.revision,
            "state_epoch": MobileContractV02.stateEpoch,
            "part_size": 16 * 1024 * 1024,
        ]
        let data = try JSONSerialization.data(withJSONObject: config)
        let json = String(decoding: data, as: UTF8.self)
        handle = databasePath.withCString { path in json.withCString { photoBackupV02R1Open(path, $0) } }
        if handle == 0 {
            throw AgentFailure.message(photoBackupV02R1LastError().map(String.init(cString:)) ?? "无法打开 Rust Agent")
        }
    }

    deinit { photoBackupV02R1Close(handle) }

    func needs(asset: String, resource: String, modifiedMs: Int64) -> Bool {
        asset.withCString { assetPointer in
            resource.withCString { resourcePointer in
                photoBackupV02R1Needs(handle, assetPointer, resourcePointer, modifiedMs)
            }
        }
    }

    func enqueue(_ input: EnqueueInput) throws {
        let json = String(decoding: try encoder.encode(input), as: UTF8.self)
        let _: String? = try call { json.withCString { photoBackupV02R1Enqueue(handle, $0) } }
    }

    func next(stagingRoot: String) throws -> PreparedJob? {
        let job: PreparedJob? = try call { stagingRoot.withCString { photoBackupV02R1Next(handle, $0) } }
        if let job {
            try MobileContractV02.requireIdentity(
                product: job.product,
                applicationVersion: job.applicationVersion,
                revision: job.revision,
                stateEpoch: job.stateEpoch
            )
        }
        return job
    }

    func markUpload(job: String, upload: String) throws {
        let _: EmptyValue? = try call {
            job.withCString { jobPointer in upload.withCString { photoBackupV02R1MarkUpload(handle, jobPointer, $0) } }
        }
    }

    func markPart(job: String, index: UInt32) throws {
        let _: EmptyValue? = try call { job.withCString { photoBackupV02R1MarkPart(handle, $0, index) } }
    }

    func markComplete(job: String) throws {
        let _: EmptyValue? = try call { job.withCString { photoBackupV02R1MarkComplete(handle, $0) } }
    }

    func markFailed(job: String, error: String) {
        let _: EmptyValue? = try? call {
            job.withCString { jobPointer in
                error.withCString { photoBackupV02R1MarkFailed(handle, jobPointer, $0, true) }
            }
        }
    }

    func statsDescription() -> String {
        let stats: JSONValue? = try? call { photoBackupV02R1Stats(handle) }
        return stats.map { String(describing: $0) } ?? ""
    }

    private func call<T: Decodable>(_ operation: () -> UnsafeMutablePointer<CChar>?) throws -> T? {
        guard let pointer = operation() else { throw AgentFailure.message("Rust Agent 返回空值") }
        defer { photoBackupV02R1StringFree(pointer) }
        let data = Data(String(cString: pointer).utf8)
        let raw = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        let expectedKeys = Set(["product", "application_version", "revision", "state_epoch", "ok", "value", "error"])
        guard raw.map({ Set($0.keys) }) == expectedKeys else {
            throw AgentFailure.message("Rust Agent 返回了未知的 v0.2 envelope")
        }
        let envelope = try decoder.decode(Envelope<T>.self, from: data)
        try MobileContractV02.requireIdentity(
            product: envelope.product,
            applicationVersion: envelope.applicationVersion,
            revision: envelope.revision,
            stateEpoch: envelope.stateEpoch
        )
        if !envelope.ok { throw AgentFailure.message(envelope.error ?? "Rust Agent 操作失败") }
        return envelope.value
    }
}

enum AgentFailure: Error { case message(String) }
