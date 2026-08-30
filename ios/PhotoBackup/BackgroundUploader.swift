import Foundation
import UIKit

private struct CreateUploadResponse: Decodable {
    let disposition: String
    let uploadId: String?
    let missingParts: [UInt32]
}

private struct BootstrapResponse: Decodable {
    let bearerToken: String
}

final class BackgroundUploader: NSObject, URLSessionTaskDelegate {
    private let agent: RustAgent
    private let serverURL: URL
    private let token: String
    private var session: URLSession!
    private let decoder: JSONDecoder = {
        let value = JSONDecoder()
        value.keyDecodingStrategy = .convertFromSnakeCase
        return value
    }()
    private let encoder: JSONEncoder = {
        let value = JSONEncoder()
        value.keyEncodingStrategy = .convertToSnakeCase
        return value
    }()

    init(agent: RustAgent, serverURL: URL, token: String) {
        self.agent = agent
        self.serverURL = serverURL
        self.token = token
        super.init()
        let configuration = URLSessionConfiguration.background(withIdentifier: "com.example.photobackup.upload")
        configuration.sessionSendsLaunchEvents = true
        configuration.isDiscretionary = false
        configuration.waitsForConnectivity = true
        session = URLSession(configuration: configuration, delegate: self, delegateQueue: nil)
    }

    func submit(_ job: PreparedJob) async throws {
        var request = authorized(path: "/v2/uploads", method: "POST")
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try encoder.encode(job.request)
        let (data, response) = try await URLSession.shared.data(for: request)
        try requireSuccess(response, data: data)
        let created = try decoder.decode(CreateUploadResponse.self, from: data)
        if created.disposition == "complete" {
            try agent.markComplete(job: job.jobId)
            return
        }
        guard let uploadId = created.uploadId else { throw UploadFailure.invalidResponse }
        try agent.markUpload(job: job.jobId, upload: uploadId)
        let localParts = Dictionary(uniqueKeysWithValues: job.localParts.map { ($0.index, $0.path) })
        if created.missingParts.isEmpty {
            try await finish(jobId: job.jobId, uploadId: uploadId)
            return
        }
        for index in created.missingParts {
            guard let path = localParts[index] else { throw UploadFailure.missingPart(index) }
            var partRequest = authorized(path: "/v2/uploads/\(uploadId)/parts/\(index)", method: "PUT")
            partRequest.setValue("application/octet-stream", forHTTPHeaderField: "Content-Type")
            let task = session.uploadTask(with: partRequest, fromFile: URL(fileURLWithPath: path))
            task.taskDescription = "\(job.jobId)|\(uploadId)|\(index)"
            task.resume()
        }
    }

    func syncAlbum(id: String, name: String, assetIds: Set<String>) async throws {
        var request = authorized(path: "/v2/albums", method: "POST")
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONSerialization.data(withJSONObject: [
            "source_album_id": id,
            "name": name,
            "source_asset_ids": Array(assetIds),
            "replace_members": true,
        ])
        let (data, response) = try await URLSession.shared.data(for: request)
        try requireSuccess(response, data: data)
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        guard let description = task.taskDescription else { return }
        let fields = description.split(separator: "|").map(String.init)
        guard fields.count == 3, let index = UInt32(fields[2]) else { return }
        let jobId = fields[0]
        let uploadId = fields[1]
        if let error {
            agent.markFailed(job: jobId, error: error.localizedDescription)
            return
        }
        guard let response = task.response as? HTTPURLResponse, (200..<300).contains(response.statusCode) else {
            agent.markFailed(job: jobId, error: "后台上传返回非成功状态")
            return
        }
        do { try agent.markPart(job: jobId, index: index) }
        catch { agent.markFailed(job: jobId, error: error.localizedDescription) }
        session.getAllTasks { [weak self] tasks in
            guard !tasks.contains(where: { $0.taskDescription?.hasPrefix("\(jobId)|") == true }) else { return }
            Task {
                do { try await self?.finish(jobId: jobId, uploadId: uploadId) }
                catch { self?.agent.markFailed(job: jobId, error: error.localizedDescription) }
            }
        }
    }

    private func finish(jobId: String, uploadId: String) async throws {
        let request = authorized(path: "/v2/uploads/\(uploadId)/complete", method: "POST")
        let (data, response) = try await URLSession.shared.data(for: request)
        try requireSuccess(response, data: data)
        try agent.markComplete(job: jobId)
    }

    private func authorized(path: String, method: String) -> URLRequest {
        var request = URLRequest(url: serverURL.appending(path: path))
        request.httpMethod = method
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        return request
    }

    private func requireSuccess(_ response: URLResponse, data: Data) throws {
        guard let http = response as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            throw UploadFailure.server(String(decoding: data, as: UTF8.self))
        }
    }

    static func bootstrap(serverURL: URL, username: String, password: String) async throws -> String {
        var request = URLRequest(url: serverURL.appending(path: "/v2/auth/bootstrap"))
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONSerialization.data(withJSONObject: [
            "username": username,
            "password": password,
            "device_name": UIDevice.current.name,
            "platform": "ios",
        ])
        let (data, response) = try await URLSession.shared.data(for: request)
        guard let http = response as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            throw UploadFailure.server(String(decoding: data, as: UTF8.self))
        }
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return try decoder.decode(BootstrapResponse.self, from: data).bearerToken
    }
}

enum UploadFailure: Error {
    case invalidResponse
    case missingPart(UInt32)
    case server(String)
}
