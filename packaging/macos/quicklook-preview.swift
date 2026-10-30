import AppKit
import Foundation
import UniformTypeIdentifiers
import QuickLookUI
import WebKit

private let maxCentralDirectoryBytes = 16 * 1024 * 1024
private let maxEntries = 50_000
private let displayedEntries = 200
private let endOfCentralDirectorySignature: UInt32 = 0x06054b50
private let zip64LocatorSignature: UInt32 = 0x07064b50
private let zip64EndSignature: UInt32 = 0x06064b50
private let centralFileHeaderSignature: UInt32 = 0x02014b50

struct ZipEntry {
    let name: String
    let size: UInt64
    let encrypted: Bool

    var isDirectory: Bool { name.hasSuffix("/") }
}

enum PreviewFailure: Error {
    case invalidArchive
    case unsupportedArchive
    case metadataLimit
}

struct ZipIndex {
    let entries: [ZipEntry]
    let totalEntries: UInt64
    let totalUncompressedSize: UInt64

    static func read(_ url: URL) throws -> ZipIndex {
        let handle = try FileHandle(forReadingFrom: url)
        defer { try? handle.close() }

        let fileSize = try handle.seekToEnd()
        let tailLength = Int(min(fileSize, 65_557))
        guard tailLength >= 22 else { throw PreviewFailure.invalidArchive }
        try handle.seek(toOffset: fileSize - UInt64(tailLength))
        let tail = try readExactly(handle, count: tailLength)
        guard let eocd = endOfCentralDirectory(in: tail),
              let commentLength = u16(tail, eocd + 20),
              eocd + 22 + Int(commentLength) == tail.count,
              let disk = u16(tail, eocd + 4),
              let centralDisk = u16(tail, eocd + 6),
              let entriesOnDisk = u16(tail, eocd + 8),
              let totalEntries16 = u16(tail, eocd + 10),
              let centralSize32 = u32(tail, eocd + 12),
              let centralOffset32 = u32(tail, eocd + 16)
        else {
            throw PreviewFailure.invalidArchive
        }
        guard disk == 0, centralDisk == 0 else { throw PreviewFailure.unsupportedArchive }
        guard entriesOnDisk == totalEntries16 else { throw PreviewFailure.unsupportedArchive }

        var totalEntries = UInt64(totalEntries16)
        var centralSize = UInt64(centralSize32)
        var centralOffset = UInt64(centralOffset32)
        var directoryBoundary = fileSize - UInt64(tailLength) + UInt64(eocd)
        if totalEntries16 == UInt16.max || centralSize32 == UInt32.max || centralOffset32 == UInt32.max {
            let absoluteEocd = directoryBoundary
            guard absoluteEocd >= 20 else { throw PreviewFailure.invalidArchive }
            let locatorOffset = absoluteEocd - 20
            try handle.seek(toOffset: absoluteEocd - 20)
            let locator = try readExactly(handle, count: 20)
            guard u32(locator, 0) == zip64LocatorSignature,
                  u32(locator, 4) == 0,
                  let zip64Offset = u64(locator, 8),
                  u32(locator, 16) == 1
            else {
                throw PreviewFailure.unsupportedArchive
            }
            guard zip64Offset <= fileSize, fileSize - zip64Offset >= 56 else {
                throw PreviewFailure.invalidArchive
            }
            try handle.seek(toOffset: zip64Offset)
            let zip64 = try readExactly(handle, count: 56)
            guard u32(zip64, 0) == zip64EndSignature,
                  let zip64RecordSize = u64(zip64, 4),
                  zip64RecordSize >= 44,
                  zip64Offset <= locatorOffset,
                  locatorOffset - zip64Offset >= 12,
                  zip64RecordSize <= locatorOffset - zip64Offset - 12,
                  u32(zip64, 16) == 0,
                  u32(zip64, 20) == 0,
                  let diskCount = u64(zip64, 24),
                  let allCount = u64(zip64, 32),
                  diskCount == allCount,
                  let size = u64(zip64, 40),
                  let offset = u64(zip64, 48)
            else {
                throw PreviewFailure.unsupportedArchive
            }
            totalEntries = allCount
            centralSize = size
            centralOffset = offset
            directoryBoundary = zip64Offset
        }

        guard totalEntries <= UInt64(maxEntries),
              centralSize <= UInt64(maxCentralDirectoryBytes),
              centralOffset <= fileSize,
              centralOffset <= directoryBoundary,
              centralSize <= directoryBoundary - centralOffset
        else {
            throw PreviewFailure.metadataLimit
        }
        guard let count = Int(exactly: centralSize),
              let expectedCount = Int(exactly: totalEntries)
        else {
            throw PreviewFailure.metadataLimit
        }
        try handle.seek(toOffset: centralOffset)
        let directory = try readExactly(handle, count: count)
        var entries = [ZipEntry]()
        entries.reserveCapacity(min(expectedCount, displayedEntries))
        var totalSize: UInt64 = 0
        var cursor = 0
        var parsedCount = 0
        while parsedCount < expectedCount {
            guard u32(directory, cursor) == centralFileHeaderSignature,
                  let flags = u16(directory, cursor + 8),
                  let plainSize32 = u32(directory, cursor + 24),
                  let nameLength = u16(directory, cursor + 28),
                  let extraLength = u16(directory, cursor + 30),
                  let commentLength = u16(directory, cursor + 32)
            else {
                throw PreviewFailure.invalidArchive
            }
            let nameStart = cursor + 46
            let extraStart = nameStart + Int(nameLength)
            let next = extraStart + Int(extraLength) + Int(commentLength)
            guard next >= cursor, next <= directory.count else {
                throw PreviewFailure.invalidArchive
            }
            let rawName = directory.subdata(in: nameStart..<extraStart)
            let name = safeName(rawName, utf8: flags & (1 << 11) != 0)
            let size: UInt64
            if plainSize32 == UInt32.max {
                guard let largeSize = zip64UncompressedSize(
                    directory.subdata(in: extraStart..<(extraStart + Int(extraLength)))
                ) else {
                    throw PreviewFailure.invalidArchive
                }
                size = largeSize
            } else {
                size = UInt64(plainSize32)
            }
            totalSize = totalSize.addingReportingOverflow(size).overflow ? UInt64.max : totalSize + size
            if entries.count < displayedEntries {
                entries.append(ZipEntry(name: name, size: size, encrypted: flags & 1 != 0))
            }
            cursor = next
            parsedCount += 1
        }
        return ZipIndex(
            entries: entries,
            totalEntries: totalEntries,
            totalUncompressedSize: totalSize
        )
    }

    private static func readExactly(_ handle: FileHandle, count: Int) throws -> Data {
        guard count >= 0, let data = try handle.read(upToCount: count), data.count == count else {
            throw PreviewFailure.invalidArchive
        }
        return data
    }

    private static func endOfCentralDirectory(in data: Data) -> Int? {
        guard data.count >= 4 else { return nil }
        for offset in stride(from: data.count - 4, through: 0, by: -1) {
            if u32(data, offset) == endOfCentralDirectorySignature,
               let commentLength = u16(data, offset + 20),
               offset + 22 + Int(commentLength) == data.count {
                return offset
            }
        }
        return nil
    }

    private static func u16(_ data: Data, _ offset: Int) -> UInt16? {
        guard offset >= 0, offset <= data.count - 2 else { return nil }
        return UInt16(data[offset]) | UInt16(data[offset + 1]) << 8
    }

    private static func u32(_ data: Data, _ offset: Int) -> UInt32? {
        guard offset >= 0, offset <= data.count - 4 else { return nil }
        return UInt32(data[offset]) | UInt32(data[offset + 1]) << 8
            | UInt32(data[offset + 2]) << 16 | UInt32(data[offset + 3]) << 24
    }

    private static func u64(_ data: Data, _ offset: Int) -> UInt64? {
        guard let low = u32(data, offset), let high = u32(data, offset + 4) else { return nil }
        return UInt64(low) | UInt64(high) << 32
    }

    private static func zip64UncompressedSize(_ extra: Data) -> UInt64? {
        var cursor = 0
        while cursor <= extra.count - 4 {
            guard let tag = u16(extra, cursor), let length = u16(extra, cursor + 2) else { return nil }
            let start = cursor + 4
            let end = start + Int(length)
            guard end >= start, end <= extra.count else { return nil }
            if tag == 0x0001 { return u64(extra, start) }
            cursor = end
        }
        return nil
    }

    private static func safeName(_ bytes: Data, utf8: Bool) -> String {
        let decoded = utf8
            ? String(data: bytes, encoding: .utf8)
            : String(data: bytes, encoding: .macOSRoman)
        return (decoded ?? String(decoding: bytes, as: UTF8.self)).unicodeScalars.map { scalar in
            CharacterSet.controlCharacters.contains(scalar) ? "�" : String(scalar)
        }.joined()
    }
}

final class PreviewController: NSViewController, QLPreviewingController {
    private var previewWebView: WKWebView!

    override func loadView() {
        let webView = WKWebView(frame: NSRect(x: 0, y: 0, width: 860, height: 640))
        webView.autoresizingMask = [.width, .height]
        previewWebView = webView
        view = webView
    }

    func preparePreviewOfFile(at url: URL, completionHandler: @escaping (Error?) -> Void) {
        DispatchQueue.global(qos: .userInitiated).async {
            let accessed = url.startAccessingSecurityScopedResource()
            defer { if accessed { url.stopAccessingSecurityScopedResource() } }
            let html: String
            do {
                let index = try ZipIndex.read(url)
                html = render(index, title: url.lastPathComponent)
            } catch {
                html = renderFailure(error, title: url.lastPathComponent)
            }
            DispatchQueue.main.async {
                self.previewWebView.loadHTMLString(html, baseURL: nil)
                completionHandler(nil)
            }
        }
    }
}

func render(_ index: ZipIndex, title: String) -> String {
    let title = escapeHTML(title)
    let rows = index.entries.map { entry in
        let kind = entry.isDirectory ? localized("folder") : localized("file")
        let lock = entry.encrypted ? " 🔒" : ""
        return "<tr><td>\(escapeHTML(entry.name))\(lock)</td><td>\(kind)</td><td class=number>\(ByteCountFormatter.string(fromByteCount: Int64(clamping: entry.size), countStyle: .file))</td></tr>"
    }.joined()
    let omitted = index.totalEntries > UInt64(index.entries.count)
        ? "<p class=muted>\(localized("omitted"))</p>" : ""
    return """
    <!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width">
    <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'">
    <style>
    :root{color-scheme:light dark}body{font:13px -apple-system,BlinkMacSystemFont,sans-serif;color:CanvasText;background:Canvas;margin:24px}
    h1{font-size:18px;margin:0 0 8px;word-break:break-all}.summary,.muted{color:GrayText}.summary{margin:0 0 18px}
    table{border-collapse:collapse;width:100%}th,td{text-align:left;border-bottom:1px solid GrayText;padding:7px 8px}
    th{color:GrayText;font-weight:500}td:first-child{overflow-wrap:anywhere}.number{font-family:ui-monospace,monospace;text-align:right;white-space:nowrap}
    </style></head><body><h1>\(title)</h1>
    <p class=summary>\(localized("entries")): \(index.totalEntries.formatted()) · \(localized("size")): \(ByteCountFormatter.string(fromByteCount: Int64(clamping: index.totalUncompressedSize), countStyle: .file))</p>
    <table><thead><tr><th>\(localized("name"))</th><th>\(localized("type"))</th><th class=number>\(localized("size"))</th></tr></thead><tbody>\(rows)</tbody></table>\(omitted)</body></html>
    """
}

private func renderFailure(_ error: Error, title: String) -> String {
    let message: String
    switch error {
    case PreviewFailure.metadataLimit: message = localized("limited")
    case PreviewFailure.unsupportedArchive: message = localized("unsupported")
    default: message = localized("unavailable")
    }
    return "<!doctype html><html><meta charset=utf-8><body><h2>\(escapeHTML(title))</h2><p>\(message)</p></body></html>"
}

private func localized(_ key: String) -> String {
    NSLocalizedString(key, bundle: .main, comment: "Quick Look archive summary")
}

private func escapeHTML(_ value: String) -> String {
    value.replacingOccurrences(of: "&", with: "&amp;")
        .replacingOccurrences(of: "<", with: "&lt;")
        .replacingOccurrences(of: ">", with: "&gt;")
        .replacingOccurrences(of: "\"", with: "&quot;")
        .replacingOccurrences(of: "'", with: "&#39;")
}
