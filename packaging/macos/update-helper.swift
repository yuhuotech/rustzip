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

func elevate(helper: URL, app: URL, dmg: URL, version: String, language: String, uid: uid_t) throws {
    let command = [
        helper.path, "--elevated", "--app", app.path, "--dmg", dmg.path,
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

func updatedApp(on mount: URL, bundleID: String) throws -> URL {
    let children = try FileManager.default.contentsOfDirectory(at: mount, includingPropertiesForKeys: nil)
    for child in children where child.pathExtension == "app" {
        if (try? bundleValue(child, "CFBundleIdentifier")) == bundleID {
            return child
        }
    }
    throw fail("The downloaded disk image does not contain RustZIP.")
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

func install(app originalApp: URL, dmg: URL, version: String, language: String, uid: uid_t, elevated: Bool) throws {
    let app = originalApp.standardizedFileURL.resolvingSymlinksInPath()
    let dmg = dmg.standardizedFileURL.resolvingSymlinksInPath()
    let bundleID = try bundleValue(app, "CFBundleIdentifier")
    guard bundleID == "com.yuhuotech.rustzip" else {
        throw fail("The current app is not RustZIP.")
    }
    let expectedTeam = try signingTeam(app)
    try validateSignature(app, expectedTeam: expectedTeam)

    let parent = app.deletingLastPathComponent()
    if !elevated && !isWritable(parent) {
        let helper = URL(fileURLWithPath: CommandLine.arguments[0]).standardizedFileURL
        try elevate(helper: helper, app: app, dmg: dmg, version: version, language: language, uid: uid)
        return
    }

    let mount = try mountImage(dmg)
    defer {
        _ = try? run("/usr/bin/hdiutil", ["detach", "-quiet", mount.path])
        try? FileManager.default.removeItem(at: mount)
    }

    let source = try updatedApp(on: mount, bundleID: bundleID)
    guard try bundleValue(source, "CFBundleShortVersionString") == version else {
        throw fail("The downloaded app version does not match the signed update manifest.")
    }
    try validateSignature(source, expectedTeam: expectedTeam)

    let id = UUID().uuidString
    let staged = parent.appendingPathComponent(".RustZIP-update-\(id).app")
    let backup = parent.appendingPathComponent(".RustZIP-backup-\(id).app")
    var originalMoved = false
    var replacementMoved = false
    defer { try? FileManager.default.removeItem(at: staged) }

    try run("/usr/bin/ditto", ["--rsrc", "--extattr", "--acl", source.path, staged.path])
    try validateSignature(staged, expectedTeam: expectedTeam)
    guard try bundleValue(staged, "CFBundleShortVersionString") == version else {
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
        let dmgPath = argument("--dmg"),
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
    let dmg = URL(fileURLWithPath: dmgPath)
    let downloadDirectory = dmg.deletingLastPathComponent()
    defer { try? FileManager.default.removeItem(at: downloadDirectory) }
    if !elevated { try waitForExit(pid_t(pidValue)) }
    do {
        try install(app: app, dmg: dmg, version: version, language: language, uid: uid, elevated: elevated)
    } catch {
        if !elevated {
            showFailure(error.localizedDescription, app: app, language: language, uid: uid, elevated: false)
        }
        throw error
    }
} catch {
    fputs("RustZIP update failed.\n", stderr)
    exit(EXIT_FAILURE)
}
