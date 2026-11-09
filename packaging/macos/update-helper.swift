import Darwin
import Foundation

struct UpdateFailure: Error, LocalizedError {
    let message: String
    var errorDescription: String? { message }
}

func fail(_ message: String) -> UpdateFailure {
    UpdateFailure(message: message)
}

@discardableResult
func run(_ executable: String, _ arguments: [String], captureOutput: Bool = false) throws -> String {
    let process = Process()
    let output: Pipe? = captureOutput ? Pipe() : nil
    process.executableURL = URL(fileURLWithPath: executable)
    process.arguments = arguments
    process.standardOutput = output ?? FileHandle.nullDevice
    process.standardError = output ?? FileHandle.nullDevice
    try process.run()
    let data = output?.fileHandleForReading.readDataToEndOfFile() ?? Data()
    process.waitUntilExit()
    let text = String(data: data, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    guard process.terminationStatus == 0 else {
        throw fail(text.isEmpty ? "A required macOS update operation failed." : text)
    }
    return text
}

func bundleValue(_ app: URL, _ key: String) throws -> String {
    let info = app.appendingPathComponent("Contents/Info.plist")
    let data = try Data(contentsOf: info)
    guard
        let dictionary = try PropertyListSerialization.propertyList(from: data, format: nil) as? [String: Any],
        let value = dictionary[key] as? String
    else {
        throw fail("The downloaded app is missing required bundle metadata.")
    }
    return value
}

func signingTeam(_ app: URL) throws -> String? {
    let details = try run("/usr/bin/codesign", ["-dv", "--verbose=4", app.path], captureOutput: true)
    guard let line = details.split(separator: "\n").first(where: { $0.hasPrefix("TeamIdentifier=") }) else {
        return nil
    }
    let value = String(line.dropFirst("TeamIdentifier=".count))
    return value == "not set" || value.isEmpty ? nil : value
}

func validateSignature(_ app: URL, expectedTeam: String?) throws {
    try run("/usr/bin/codesign", ["--verify", "--deep", "--strict", app.path], captureOutput: true)
    let actualTeam = try signingTeam(app)
    guard actualTeam == expectedTeam else {
        throw fail("The downloaded app was signed by a different Apple Developer account.")
    }
}

func shellQuote(_ value: String) -> String {
    "'" + value.replacingOccurrences(of: "'", with: "'\\''") + "'"
}

func appleScriptQuote(_ value: String) -> String {
    value.replacingOccurrences(of: "\\", with: "\\\\")
        .replacingOccurrences(of: "\"", with: "\\\"")
}

func helperText(_ key: Int, app: URL, language: String) -> String {
    let strings = app.appendingPathComponent("Contents/Resources/update-helper-strings.tsv")
    guard
        let contents = try? String(contentsOf: strings, encoding: .utf8),
        let row = contents.split(whereSeparator: \.isNewline).first(where: {
            let columns = $0.split(separator: "\t", omittingEmptySubsequences: false)
            return columns.count == 3 && columns[0] == Substring(language)
        })
    else {
        return key == 1 ? "RustZIP needs permission to update its copy in Applications." : "RustZIP could not complete the update: {detail}"
    }
    let columns = row.split(separator: "\t", omittingEmptySubsequences: false)
    return String(columns[key])
}

func isWritable(_ directory: URL) -> Bool {
    access(directory.path, W_OK) == 0
}

func elevate(helper: URL, app: URL, archive: URL, format: String, sha256: String,
             source: URL, version: String, language: String, uid: uid_t) throws {
    let command = [
        helper.path, "--elevated", "--app", app.path, "--archive", archive.path,
        "--format", format, "--sha256", sha256, "--prepared-app", source.path,
        "--version", version, "--language", language, "--pid", "0", "--uid", String(uid)
    ].map(shellQuote).joined(separator: " ")
    let prompt = appleScriptQuote(helperText(1, app: app, language: language))
    let script = "do shell script \"\(appleScriptQuote(command))\" with administrator privileges with prompt \"\(prompt)\""
    try run("/usr/bin/osascript", ["-e", script])
}

func mountImage(_ dmg: URL) throws -> URL {
    let mount = URL(fileURLWithPath: "/private/tmp/RustZIPUpdate-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: mount, withIntermediateDirectories: false)
    do {
        try run("/usr/bin/hdiutil", ["attach", "-readonly", "-nobrowse", "-quiet", "-mountpoint", mount.path, dmg.path])
        return mount
    } catch {
        try? FileManager.default.removeItem(at: mount)
        throw error
    }
}

func updatedApp(in directory: URL, bundleID: String) throws -> URL {
    let children = try FileManager.default.contentsOfDirectory(
        at: directory, includingPropertiesForKeys: [.isDirectoryKey, .isSymbolicLinkKey])
    var match: URL?
    for child in children where child.pathExtension == "app" {
        let values = try child.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey])
        if values.isDirectory == true && values.isSymbolicLink != true
            && (try? bundleValue(child, "CFBundleIdentifier")) == bundleID {
            guard match == nil else {
                throw fail("The downloaded update contains multiple RustZIP apps.")
            }
            match = child
        }
    }
    guard let app = match else {
        throw fail("The downloaded update does not contain RustZIP.")
    }
    return app
}

func verifyArchive(_ archive: URL, expectedSHA256: String) throws {
    let expected = expectedSHA256.lowercased()
    guard expected.count == 64,
          expected.unicodeScalars.allSatisfy({ CharacterSet(charactersIn: "0123456789abcdef").contains($0) })
    else {
        throw fail("The update checksum is invalid.")
    }
    let output = try run("/usr/bin/shasum", ["-a", "256", archive.path], captureOutput: true)
    guard output.split(separator: " ").first.map(String.init)?.lowercased() == expected else {
        throw fail("The downloaded update changed after signature verification.")
    }
}

func withUpdateDirectory(_ archive: URL, format: String, body: (URL) throws -> Void) throws {
    switch format {
    case "dmg":
        let mount = try mountImage(archive)
        defer {
            _ = try? run("/usr/bin/hdiutil", ["detach", "-quiet", mount.path])
            try? FileManager.default.removeItem(at: mount)
        }
        try body(mount)
    case "zip":
        let directory = archive.deletingLastPathComponent()
            .appendingPathComponent("extracted-\(UUID().uuidString)")
        try FileManager.default.createDirectory(
            at: directory, withIntermediateDirectories: false,
            attributes: [.posixPermissions: 0o700])
        defer { try? FileManager.default.removeItem(at: directory) }
        try run("/usr/bin/ditto", ["-x", "-k", archive.path, directory.path])
        try body(directory)
    default:
        throw fail("The update archive format is unsupported.")
    }
}

func waitForExit(_ pid: pid_t) throws {
    while true {
        if kill(pid, 0) != 0 && errno == ESRCH { return }
        usleep(100_000)
    }
}

func openInstalledApp(_ app: URL, uid: uid_t, elevated: Bool) throws {
    if elevated {
        try run("/bin/launchctl", ["asuser", String(uid), "/usr/bin/open", "-a", app.path])
    } else {
        try run("/usr/bin/open", ["-a", app.path])
    }
}

func installPreparedApp(_ source: URL, app: URL, bundleID: String, expectedTeam: String?,
                        version: String, uid: uid_t, elevated: Bool) throws {
    let parent = app.deletingLastPathComponent()
    guard try bundleValue(source, "CFBundleIdentifier") == bundleID,
          try bundleValue(source, "CFBundleShortVersionString") == version else {
        throw fail("The downloaded app version does not match the signed update manifest.")
    }
    try validateSignature(source, expectedTeam: expectedTeam)

    let id = UUID().uuidString
    let stagingDirectory = parent.appendingPathComponent(".RustZIP-update-\(id)")
    try FileManager.default.createDirectory(
        at: stagingDirectory, withIntermediateDirectories: false,
        attributes: [.posixPermissions: 0o700])
    let staged = stagingDirectory.appendingPathComponent("RustZIP.app")
    let backup = parent.appendingPathComponent(".RustZIP-backup-\(id).app")
    var originalMoved = false
    var replacementMoved = false
    defer { try? FileManager.default.removeItem(at: stagingDirectory) }

    try run("/usr/bin/ditto", ["--rsrc", "--extattr", "--acl", source.path, staged.path])
    try validateSignature(staged, expectedTeam: expectedTeam)
    guard try bundleValue(staged, "CFBundleIdentifier") == bundleID,
          try bundleValue(staged, "CFBundleShortVersionString") == version else {
        throw fail("The staged app version changed during installation.")
    }

    do {
        try FileManager.default.moveItem(at: app, to: backup)
        originalMoved = true
        try FileManager.default.moveItem(at: staged, to: app)
        replacementMoved = true
        try openInstalledApp(app, uid: uid, elevated: elevated)
        try? FileManager.default.removeItem(at: backup)
    } catch {
        if replacementMoved { try? FileManager.default.moveItem(at: app, to: staged) }
        if originalMoved { try? FileManager.default.moveItem(at: backup, to: app) }
        throw error
    }
}

func install(app originalApp: URL, archive originalArchive: URL, format: String, sha256: String,
             preparedApp: URL?, version: String, language: String, uid: uid_t, elevated: Bool) throws {
    let app = originalApp.standardizedFileURL.resolvingSymlinksInPath()
    let archive = originalArchive.standardizedFileURL.resolvingSymlinksInPath()
    let bundleID = try bundleValue(app, "CFBundleIdentifier")
    #if RUSTZIP_UPDATER_TESTING
    let expectedBundleID = "com.yuhuotech.rustzip.updater-smoke"
    #else
    let expectedBundleID = "com.yuhuotech.rustzip"
    #endif
    guard bundleID == expectedBundleID else {
        throw fail("The current app is not RustZIP.")
    }
    let expectedTeam = try signingTeam(app)
    try validateSignature(app, expectedTeam: expectedTeam)

    if let source = preparedApp {
        guard elevated && geteuid() == 0 else {
            throw fail("Only the authorized installer can use a prepared app.")
        }
        try installPreparedApp(source, app: app, bundleID: bundleID, expectedTeam: expectedTeam,
                               version: version, uid: uid, elevated: true)
        return
    }
    guard !elevated && geteuid() != 0 else {
        throw fail("Update archives must be prepared without administrator privileges.")
    }

    try verifyArchive(archive, expectedSHA256: sha256)
    try withUpdateDirectory(archive, format: format) { directory in
        let source = try updatedApp(in: directory, bundleID: bundleID)
        guard try bundleValue(source, "CFBundleShortVersionString") == version else {
            throw fail("The downloaded app version does not match the signed update manifest.")
        }
        try validateSignature(source, expectedTeam: expectedTeam)
        if !isWritable(app.deletingLastPathComponent()) {
            let helper = URL(fileURLWithPath: CommandLine.arguments[0]).standardizedFileURL
            try elevate(helper: helper, app: app, archive: archive, format: format,
                        sha256: sha256, source: source, version: version, language: language, uid: uid)
        } else {
            try installPreparedApp(source, app: app, bundleID: bundleID, expectedTeam: expectedTeam,
                                   version: version, uid: uid, elevated: false)
        }
    }
}

func showFailure(_ message: String, app: URL, language: String, uid: uid_t, elevated: Bool) {
    let summary = message.split(whereSeparator: \.isNewline).joined(separator: " ").prefix(500)
    let failure = helperText(2, app: app, language: language)
        .replacingOccurrences(of: "{detail}", with: String(summary))
    let script = "display dialog \"\(appleScriptQuote(failure))\" buttons {\"OK\"} default button 1 with title \"RustZIP\""
    if elevated {
        _ = try? run("/bin/launchctl", ["asuser", String(uid), "/usr/bin/osascript", "-e", script])
        _ = try? run("/bin/launchctl", ["asuser", String(uid), "/usr/bin/open", "-a", app.path])
    } else {
        _ = try? run("/usr/bin/osascript", ["-e", script])
        _ = try? run("/usr/bin/open", ["-a", app.path])
    }
}

func argument(_ name: String) -> String? {
    guard let index = CommandLine.arguments.firstIndex(of: name), index + 1 < CommandLine.arguments.count else {
        return nil
    }
    return CommandLine.arguments[index + 1]
}

do {
    guard
        let appPath = argument("--app"),
        let archivePath = argument("--archive"),
        let format = argument("--format"),
        let sha256 = argument("--sha256"),
        let version = argument("--version"),
        let language = argument("--language"),
        let pidText = argument("--pid"),
        let pidValue = Int32(pidText)
    else {
        throw fail("The update helper received invalid arguments.")
    }
    let elevated = CommandLine.arguments.contains("--elevated")
    let uid = argument("--uid").flatMap(UInt32.init) ?? getuid()
    let app = URL(fileURLWithPath: appPath)
    let archive = URL(fileURLWithPath: archivePath)
    let preparedApp = argument("--prepared-app").map { URL(fileURLWithPath: $0) }
    let downloadDirectory = archive.deletingLastPathComponent()
    defer { if !elevated { try? FileManager.default.removeItem(at: downloadDirectory) } }
    if !elevated { try waitForExit(pid_t(pidValue)) }
    do {
        try install(app: app, archive: archive, format: format, sha256: sha256,
                    preparedApp: preparedApp, version: version, language: language, uid: uid, elevated: elevated)
    } catch {
        #if !RUSTZIP_UPDATER_TESTING
        if !elevated {
            showFailure(error.localizedDescription, app: app, language: language, uid: uid, elevated: false)
        }
        #endif
        throw error
    }
} catch {
    fputs("RustZIP update failed.\n", stderr)
    exit(EXIT_FAILURE)
}
