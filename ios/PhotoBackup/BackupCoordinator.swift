import BackgroundTasks
import Foundation
import Photos

@MainActor
final class BackupCoordinator: ObservableObject {
    static let shared = BackupCoordinator()

    @Published var serverURL = UserDefaults.standard.string(forKey: "server_url") ?? ""
    @Published var username = KeychainStore.load("username") ?? ""
    @Published var password = KeychainStore.load("password") ?? ""
    @Published var status = "等待配置"
    @Published var running = false

    private var uploader: BackgroundUploader?

    func runBackup() async {
        guard !running else { return }
        running = true
        defer { running = false }
        do {
            guard let baseURL = URL(string: serverURL), baseURL.scheme == "https" else {
                throw CoordinatorFailure.message("请输入有效的 HTTPS 地址")
            }
            let normalizedUsername = username.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !normalizedUsername.isEmpty, !password.isEmpty else {
                throw CoordinatorFailure.message("请输入登录账号和密码")
            }
            let normalizedURL = serverURL.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
            let credentialsChanged = UserDefaults.standard.string(forKey: "server_url") != normalizedURL
                || KeychainStore.load("username") != normalizedUsername
                || KeychainStore.load("password") != password
            UserDefaults.standard.set(normalizedURL, forKey: "server_url")
            username = normalizedUsername
            try KeychainStore.save(normalizedUsername, for: "username")
            try KeychainStore.save(password, for: "password")
            if credentialsChanged { KeychainStore.delete("bearer_token") }
            var bearer = KeychainStore.load("bearer_token")
            if bearer == nil {
                status = "正在登录并注册设备"
                bearer = try await BackgroundUploader.bootstrap(serverURL: baseURL, username: normalizedUsername, password: password)
                try KeychainStore.save(bearer!, for: "bearer_token")
            }
            let masterKey = try KeychainStore.randomKey(named: "account_master_key")
            let dedupeKey = try KeychainStore.randomKey(named: "account_dedupe_key")
            let support = try FileManager.default.url(
                for: .applicationSupportDirectory,
                in: .userDomainMask,
                appropriateFor: nil,
                create: true
            )
            let staging = support.appendingPathComponent("backup-staging", isDirectory: true)
            try FileManager.default.createDirectory(at: staging, withIntermediateDirectories: true)
            let agent = try RustAgent(
                databasePath: support.appendingPathComponent("agent.sqlite").path,
                masterKey: masterKey,
                dedupeKey: dedupeKey
            )
            let authorization = await PhotoScanner().requestAccess()
            guard authorization == .authorized || authorization == .limited else {
                throw CoordinatorFailure.message("没有可用的照片访问权限")
            }
            status = authorization == .limited ? "正在扫描已选择的照片" : "正在扫描照片库"
            let queued = try await PhotoScanner().scan(agent: agent, stagingRoot: staging)
            status = "发现 \(queued) 个待处理资源"
            let uploader = BackgroundUploader(agent: agent, serverURL: baseURL, token: bearer!)
            self.uploader = uploader
            for _ in 0..<24 {
                guard let job = try agent.next(stagingRoot: staging.path) else { break }
                try await uploader.submit(job)
            }
            scheduleBackgroundRun()
            status = "上传任务已交给系统；应用可进入后台"
        } catch {
            status = "备份失败：\(error.localizedDescription)"
        }
    }

    private func scheduleBackgroundRun() {
        let request = BGProcessingTaskRequest(identifier: "com.example.photobackup.processing")
        request.requiresNetworkConnectivity = true
        request.requiresExternalPower = false
        request.earliestBeginDate = Date(timeIntervalSinceNow: 15 * 60)
        try? BGTaskScheduler.shared.submit(request)
    }
}

enum CoordinatorFailure: Error {
    case message(String)
}
