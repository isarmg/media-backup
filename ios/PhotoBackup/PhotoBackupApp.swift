import BackgroundTasks
import SwiftUI

@main
struct PhotoBackupApp: App {
    @UIApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate

    var body: some Scene {
        WindowGroup {
            ContentView()
                .environmentObject(BackupCoordinator.shared)
        }
    }
}

final class AppDelegate: NSObject, UIApplicationDelegate {
    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil
    ) -> Bool {
        BGTaskScheduler.shared.register(
            forTaskWithIdentifier: MobileContractV02.processingTask,
            using: nil
        ) { task in
            guard let processing = task as? BGProcessingTask else { return }
            let work = Task {
                await BackupCoordinator.shared.runBackup()
                processing.setTaskCompleted(success: true)
            }
            processing.expirationHandler = {
                work.cancel()
                processing.setTaskCompleted(success: false)
            }
        }
        return true
    }
}
