import AppKit
import Foundation
import QuickLookUI

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

final class PreviewController: NSViewController, QLPreviewingController, NSTableViewDataSource, NSTableViewDelegate {
    private var titleField: NSTextField!
    private var summaryField: NSTextField!
    private var omittedField: NSTextField!
    private var tableView: NSTableView!
    private var entries: [ZipEntry] = []

    override func loadView() {
        let root = NSView(frame: NSRect(x: 0, y: 0, width: 860, height: 640))

        titleField = NSTextField(labelWithString: "")
        titleField.font = .systemFont(ofSize: 18, weight: .semibold)
        titleField.lineBreakMode = .byTruncatingMiddle

        summaryField = NSTextField(labelWithString: "")
        summaryField.textColor = .secondaryLabelColor

        omittedField = NSTextField(labelWithString: localized("omitted"))
        omittedField.textColor = .secondaryLabelColor
        omittedField.isHidden = true

        let header = NSStackView(views: [titleField, summaryField, omittedField])
        header.orientation = .vertical
        header.alignment = .leading
        header.spacing = 6
        header.translatesAutoresizingMaskIntoConstraints = false
        root.addSubview(header)

        tableView = NSTableView(frame: NSRect(x: 0, y: 0, width: 780, height: 500))
        tableView.autoresizingMask = [.width]
        tableView.dataSource = self
        tableView.delegate = self
        tableView.rowHeight = 30
        tableView.intercellSpacing = NSSize(width: 0, height: 0)
        tableView.columnAutoresizingStyle = .lastColumnOnlyAutoresizingStyle

        let nameColumn = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("name"))
        nameColumn.title = localized("name")
        nameColumn.width = 500
        nameColumn.minWidth = 200
        tableView.addTableColumn(nameColumn)

        let typeColumn = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("type"))
        typeColumn.title = localized("type")
        typeColumn.width = 130
        typeColumn.minWidth = 80
        tableView.addTableColumn(typeColumn)

        let sizeColumn = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("size"))
        sizeColumn.title = localized("size")
        sizeColumn.width = 150
        sizeColumn.minWidth = 90
        tableView.addTableColumn(sizeColumn)

        let scrollView = NSScrollView()
        scrollView.borderType = .noBorder
        scrollView.hasVerticalScroller = true
        scrollView.hasHorizontalScroller = true
        scrollView.documentView = tableView
        scrollView.translatesAutoresizingMaskIntoConstraints = false
        root.addSubview(scrollView)

        NSLayoutConstraint.activate([
            header.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 24),
            header.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -24),
            header.topAnchor.constraint(equalTo: root.topAnchor, constant: 24),
            scrollView.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 24),
            scrollView.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -24),
            scrollView.topAnchor.constraint(equalTo: header.bottomAnchor, constant: 18),
            scrollView.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -24),
        ])

        view = root
    }

    func preparePreviewOfFile(at url: URL, completionHandler: @escaping (Error?) -> Void) {
        DispatchQueue.global(qos: .userInitiated).async {
            let accessed = url.startAccessingSecurityScopedResource()
            defer { if accessed { url.stopAccessingSecurityScopedResource() } }
            let index: ZipIndex
            do {
                index = try ZipIndex.read(url)
            } catch {
                DispatchQueue.main.async {
                    self.showFailure(error, title: url.lastPathComponent)
                    completionHandler(nil)
                }
                return
            }
            DispatchQueue.main.async {
                self.show(index: index, title: url.lastPathComponent)
                completionHandler(nil)
            }
        }
    }

    func numberOfRows(in tableView: NSTableView) -> Int {
        entries.count
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard entries.indices.contains(row), let column = tableColumn else { return nil }
        let identifier = column.identifier
        let cell = (tableView.makeView(withIdentifier: identifier, owner: self) as? NSTextField)
            ?? makeCell(identifier: identifier)
        let entry = entries[row]

        switch identifier.rawValue {
        case "name":
            cell.stringValue = entry.encrypted ? "\(entry.name) 🔒" : entry.name
            cell.alignment = .left
            cell.font = .systemFont(ofSize: NSFont.systemFontSize)
        case "type":
            cell.stringValue = localized(entry.isDirectory ? "folder" : "file")
            cell.alignment = .left
            cell.font = .systemFont(ofSize: NSFont.systemFontSize)
        case "size":
            cell.stringValue = ByteCountFormatter.string(
                fromByteCount: Int64(clamping: entry.size),
                countStyle: .file
            )
            cell.alignment = .right
            cell.font = .monospacedDigitSystemFont(ofSize: NSFont.systemFontSize, weight: .regular)
        default:
            return nil
        }
        return cell
    }

    private func show(index: ZipIndex, title: String) {
        entries = index.entries
        titleField.stringValue = title
        summaryField.stringValue = "\(localized("entries")): \(index.totalEntries.formatted()) · \(localized("size")): \(ByteCountFormatter.string(fromByteCount: Int64(clamping: index.totalUncompressedSize), countStyle: .file))"
        omittedField.isHidden = index.totalEntries <= UInt64(index.entries.count)
        tableView.isHidden = false
        tableView.reloadData()
    }

    private func showFailure(_ error: Error, title: String) {
        entries = []
        titleField.stringValue = title
        summaryField.stringValue = failureMessage(error)
        omittedField.isHidden = true
        tableView.isHidden = true
    }

    private func makeCell(identifier: NSUserInterfaceItemIdentifier) -> NSTextField {
        let cell = NSTextField(labelWithString: "")
        cell.identifier = identifier
        cell.lineBreakMode = .byTruncatingTail
        cell.usesSingleLineMode = true
        return cell
    }
}

private func failureMessage(_ error: Error) -> String {
    switch error {
    case PreviewFailure.metadataLimit: return localized("limited")
    case PreviewFailure.unsupportedArchive: return localized("unsupported")
    default: return localized("unavailable")
    }
}

private func localized(_ key: String) -> String {
    NSLocalizedString(key, bundle: .main, comment: "Quick Look archive summary")
}
