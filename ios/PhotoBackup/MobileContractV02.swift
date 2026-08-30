import Foundation

enum MobileContractV02 {
    static let product = "photo-backup"
    static let applicationVersion = "0.2.0"
    static let revision: UInt32 = 1
    static let stateEpoch = "photo-backup-mobile-v0.2-r1"

    static let databaseFilename = "agent-v0.2-r1.sqlite"
    static let stagingDirectory = "backup-staging-v0.2-r1"
    static let keychainService = "com.example.photobackup.v02.r1.keychain"
    static let preferences = UserDefaults(suiteName: "com.example.photobackup.v02.r1.preferences")!
    static let tokenKey = "bearer_token_v0_2_r1"
    static let processingTask = "com.example.photobackup.v02.processing.v0-2-r1"
    static let uploadSession = "com.example.photobackup.v02.upload.v0-2-r1"

    static func requireIdentity(
        product: String,
        applicationVersion: String,
        revision: UInt32,
        stateEpoch: String
    ) throws {
        guard product == self.product,
              applicationVersion == self.applicationVersion,
              revision == self.revision,
              stateEpoch == self.stateEpoch else {
            throw AgentFailure.message("Rust Agent 返回了非 v0.2 revision 1 合约")
        }
    }
}
