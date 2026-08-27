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
    @Published var albums: [PhotoAlbum] = []
    @Published var selectedAlbumIds: Set<String> = Set(UserDefaults.standard.stringArray(forKey: "selected_album_ids") ?? [])
    @Published var remoteAssets: [RemoteAsset] = []
    @Published var showingTrash = false
    @Published var libraryLoading = false
    @Published var duplicateGroupCount = 0
    @Published var newTagName = ""
    @Published var remoteThumbnails: [UUID: Data] = [:]

    private var uploader: BackgroundUploader?
    private var albumSelectionConfigured = UserDefaults.standard.bool(forKey: "album_selection_configured")

    func refreshAlbums() async {
        let authorization = await PhotoScanner().requestAccess()
        guard authorization == .authorized || authorization == .limited else {
            status = "没有可用的照片访问权限"
            return
        }
        albums = PhotoScanner().albums()
        if !albumSelectionConfigured {
            selectedAlbumIds = Set(albums.map(\.id))
            albumSelectionConfigured = true
            UserDefaults.standard.set(true, forKey: "album_selection_configured")
            UserDefaults.standard.set(Array(selectedAlbumIds), forKey: "selected_album_ids")
        }
    }

    func setAlbum(_ id: String, enabled: Bool) {
        if enabled { selectedAlbumIds.insert(id) } else { selectedAlbumIds.remove(id) }
        UserDefaults.standard.set(Array(selectedAlbumIds), forKey: "selected_album_ids")
        albumSelectionConfigured = true
        UserDefaults.standard.set(true, forKey: "album_selection_configured")
    }

    func refreshLibrary(trashed: Bool? = nil) async {
        let requestedTrash = trashed ?? showingTrash
        libraryLoading = true
        defer { libraryLoading = false }
        do {
            let library = try await remoteLibrary()
            remoteAssets = try await library.timeline(trashed: requestedTrash)
            let sequence = UserDefaults.standard.object(forKey: "library_sync_sequence") as? NSNumber
            let nextSequence = try await library.advanceSync(from: sequence?.int64Value ?? 0)
            UserDefaults.standard.set(nextSequence, forKey: "library_sync_sequence")
            duplicateGroupCount = try await library.duplicateGroupCount()
            var thumbnails: [UUID: Data] = [:]
            for asset in remoteAssets.prefix(24) {
                if let data = try? await library.thumbnailData(for: asset) { thumbnails[asset.id] = data }
            }
            remoteThumbnails = thumbnails
            showingTrash = requestedTrash
            status = requestedTrash ? "回收站已刷新" : "服务器时间线已刷新"
        } catch {
            status = "加载图库失败：\(error.localizedDescription)"
        }
    }

    func restoreToPhone(_ asset: RemoteAsset) async {
        do {
            let authorization = await PhotoScanner().requestAccess()
            guard authorization == .authorized || authorization == .limited else {
                throw CoordinatorFailure.message("没有写入照片图库的权限")
            }
            let filename = try await remoteLibrary().restoreToPhotos(asset: asset)
            status = "已恢复到系统照片：\(filename)"
        } catch {
            status = "恢复失败：\(error.localizedDescription)"
        }
    }

    func restoreAllToPhone() async {
        do {
            let authorization = await PhotoScanner().requestAccess()
            guard authorization == .authorized || authorization == .limited else {
                throw CoordinatorFailure.message("没有写入照片图库的权限")
            }
            let library = try await remoteLibrary()
            var restored = 0
            for asset in remoteAssets where !asset.isTrashed {
                _ = try await library.restoreToPhotos(asset: asset)
                restored += 1
                status = "正在恢复 \(restored) / \(remoteAssets.count)"
            }
            status = "已恢复 \(restored) 个媒体项目到系统照片"
        } catch {
            status = "批量恢复失败：\(error.localizedDescription)"
        }
    }

    func toggleFavorite(_ asset: RemoteAsset) async {
        do {
            try await remoteLibrary().setFavorite(asset: asset, value: !asset.favorite)
            await refreshLibrary()
        } catch {
            status = "更新收藏失败：\(error.localizedDescription)"
        }
    }

    func toggleArchived(_ asset: RemoteAsset) async {
        do {
            try await remoteLibrary().setArchived(asset: asset, value: !asset.archived)
            await refreshLibrary()
        } catch {
            status = "更新归档失败：\(error.localizedDescription)"
        }
    }

    func addTag(to asset: RemoteAsset) async {
        do {
            try await remoteLibrary().addTag(named: newTagName, to: asset)
            await refreshLibrary()
        } catch {
            status = "添加标签失败：\(error.localizedDescription)"
        }
    }

    func toggleTrash(_ asset: RemoteAsset) async {
        do {
            let library = try await remoteLibrary()
            if asset.isTrashed { try await library.restoreFromTrash(asset: asset) }
            else { try await library.trash(asset: asset) }
            await refreshLibrary()
        } catch {
            status = asset.isTrashed ? "撤销删除失败：\(error.localizedDescription)" : "移入回收站失败：\(error.localizedDescription)"
        }
    }

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
            let support = try FileManager.default.url(
                for: .applicationSupportDirectory,
                in: .userDomainMask,
                appropriateFor: nil,
                create: true
            )
            let staging = support.appendingPathComponent("backup-staging", isDirectory: true)
            try FileManager.default.createDirectory(at: staging, withIntermediateDirectories: true)
            let agent = try RustAgent(
                databasePath: support.appendingPathComponent("agent.sqlite").path
            )
            let authorization = await PhotoScanner().requestAccess()
            guard authorization == .authorized || authorization == .limited else {
                throw CoordinatorFailure.message("没有可用的照片访问权限")
            }
            status = authorization == .limited ? "正在扫描已选择的照片" : "正在扫描照片库"
            if albums.isEmpty { albums = PhotoScanner().albums() }
            if !albumSelectionConfigured {
                selectedAlbumIds = Set(albums.map(\.id))
                albumSelectionConfigured = true
                UserDefaults.standard.set(true, forKey: "album_selection_configured")
            }
            UserDefaults.standard.set(Array(selectedAlbumIds), forKey: "selected_album_ids")
            let scan = try await PhotoScanner().scan(
                agent: agent,
                stagingRoot: staging,
                selectedAlbumIds: selectedAlbumIds
            )
            status = "发现 \(scan.queued) 个待处理资源"
            let uploader = BackgroundUploader(agent: agent, serverURL: baseURL, token: bearer!)
            self.uploader = uploader
            for _ in 0..<24 {
                guard let job = try agent.next(stagingRoot: staging.path) else { break }
                try await uploader.submit(job)
            }
            for (id, album) in scan.albums {
                try await uploader.syncAlbum(id: id, name: album.name, assetIds: album.assetIds)
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

    private func remoteLibrary() async throws -> RemoteLibrary {
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
        serverURL = normalizedURL
        username = normalizedUsername
        try KeychainStore.save(normalizedUsername, for: "username")
        try KeychainStore.save(password, for: "password")
        if credentialsChanged { KeychainStore.delete("bearer_token") }
        var bearer = KeychainStore.load("bearer_token")
        if bearer == nil {
            bearer = try await BackgroundUploader.bootstrap(
                serverURL: baseURL,
                username: normalizedUsername,
                password: password
            )
            try KeychainStore.save(bearer!, for: "bearer_token")
        }
        return RemoteLibrary(serverURL: baseURL, token: bearer!)
    }
}

enum CoordinatorFailure: LocalizedError {
    case message(String)

    var errorDescription: String? {
        switch self { case .message(let value): value }
    }
}
