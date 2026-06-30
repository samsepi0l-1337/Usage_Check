import Foundation

enum UsageCheckError: Error {
    case invalidURL
}

struct UsagePaths: Sendable {
    var home: URL
    var environment: [String: String]

    init(
        home: URL = FileManager.default.homeDirectoryForCurrentUser,
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) {
        self.home = home
        self.environment = environment
    }

    var codexAuthFile: URL {
        if let codexHome = environment["CODEX_HOME"], !codexHome.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            return URL(fileURLWithPath: codexHome).appendingPathComponent("auth.json")
        }

        return home.appendingPathComponent(".codex/auth.json")
    }

    var codexSessionRoots: [URL] {
        let sessions: URL
        if let codexHome = environment["CODEX_HOME"], !codexHome.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            sessions = URL(fileURLWithPath: codexHome).appendingPathComponent("sessions")
        } else {
            sessions = home.appendingPathComponent(".codex/sessions")
        }

        let archived = sessions
            .deletingLastPathComponent()
            .appendingPathComponent("archived_sessions")

        return [sessions, archived]
    }

    var claudeCredentialFiles: [URL] {
        claudeConfigRoots.map { $0.appendingPathComponent(".credentials.json") }
    }

    var claudeProjectRoots: [URL] {
        claudeConfigRoots.map { root in
            root.lastPathComponent == "projects" ? root : root.appendingPathComponent("projects")
        }
    }

    private var claudeConfigRoots: [URL] {
        if let configDir = environment["CLAUDE_CONFIG_DIR"], !configDir.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            return configDir
                .split(separator: ",")
                .map { URL(fileURLWithPath: String($0).trimmingCharacters(in: .whitespacesAndNewlines)) }
        }

        return [
            home.appendingPathComponent(".claude"),
            home.appendingPathComponent(".config/claude")
        ]
    }

    var geminiLogRoots: [URL] {
        [
            home.appendingPathComponent(".gemini"),
            home.appendingPathComponent(".config/gemini")
        ]
    }
}

struct LineReader {
    static let maxLineLength = 512 * 1024

    static func readLines(from file: URL, _ handle: (String) throws -> Void) rethrows {
        guard let stream = InputStream(url: file) else {
            return
        }

        stream.open()
        defer { stream.close() }

        let bufferSize = 16 * 1024
        var buffer = [UInt8](repeating: 0, count: bufferSize)
        var line = Data()
        var overflowed = false

        while stream.hasBytesAvailable {
            let read = stream.read(&buffer, maxLength: bufferSize)
            if read <= 0 {
                break
            }

            for byte in buffer[..<read] {
                if byte == 10 {
                    if !overflowed, let text = String(data: line, encoding: .utf8) {
                        try handle(text)
                    }
                    line.removeAll(keepingCapacity: true)
                    overflowed = false
                    continue
                }

                if byte == 13 {
                    continue
                }

                if !overflowed {
                    if line.count < maxLineLength {
                        line.append(byte)
                    } else {
                        overflowed = true
                    }
                }
            }
        }

        if !line.isEmpty, !overflowed, let text = String(data: line, encoding: .utf8) {
            try handle(text)
        }
    }
}

enum JSONValue {
    static func parse(_ line: String) -> Any? {
        guard let data = line.data(using: .utf8) else {
            return nil
        }

        return try? JSONSerialization.jsonObject(with: data)
    }

    static func dictionary(_ value: Any?) -> [String: Any]? {
        value as? [String: Any]
    }

    static func string(_ value: Any?) -> String? {
        if let text = value as? String {
            return text
        }

        if let number = value as? NSNumber {
            return number.stringValue
        }

        return nil
    }

    static func int64(_ value: Any?) -> Int64 {
        if let number = value as? NSNumber {
            return number.int64Value
        }

        if let text = value as? String, let value = Int64(text) {
            return value
        }

        return 0
    }

    static func double(_ value: Any?) -> Double? {
        if let number = value as? NSNumber {
            return number.doubleValue
        }

        if let text = value as? String {
            return Double(text)
        }

        return nil
    }
}

struct TimestampParser {
    static func parse(_ value: Any?) -> Date? {
        if let number = value as? NSNumber {
            let raw = number.doubleValue
            if raw > 10_000_000_000 {
                return Date(timeIntervalSince1970: raw / 1000)
            }

            if raw > 0 {
                return Date(timeIntervalSince1970: raw)
            }
        }

        guard let text = JSONValue.string(value)?.trimmingCharacters(in: .whitespacesAndNewlines), !text.isEmpty else {
            return nil
        }

        let isoFormatter = ISO8601DateFormatter()
        isoFormatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        let isoNoFractionFormatter = ISO8601DateFormatter()
        isoNoFractionFormatter.formatOptions = [.withInternetDateTime]

        if let date = isoFormatter.date(from: text) ?? isoNoFractionFormatter.date(from: text) {
            return date
        }

        let fallback = DateFormatter()
        fallback.locale = Locale(identifier: "en_US_POSIX")
        fallback.dateFormat = "yyyy-MM-dd'T'HH:mm:ssZ"
        return fallback.date(from: text)
    }

    static func find(in dictionary: [String: Any]) -> Date? {
        for key in ["timestamp", "created_at", "createdAt", "time", "date"] {
            if let date = parse(dictionary[key]) {
                return date
            }
        }

        return nil
    }
}

struct FileEnumerator {
    static func jsonlFiles(roots: [URL], modifiedSince cutoff: Date) -> [URL] {
        roots.flatMap { jsonlFiles(root: $0, modifiedSince: cutoff) }
    }

    static func jsonlFiles(root: URL, modifiedSince cutoff: Date) -> [URL] {
        let manager = FileManager.default
        var isDirectory: ObjCBool = false
        guard manager.fileExists(atPath: root.path, isDirectory: &isDirectory), isDirectory.boolValue else {
            return []
        }

        let keys: [URLResourceKey] = [.isRegularFileKey, .contentModificationDateKey]
        guard let enumerator = manager.enumerator(
            at: root,
            includingPropertiesForKeys: keys,
            options: [.skipsPackageDescendants]
        ) else {
            return []
        }

        return enumerator.compactMap { item in
            guard let url = item as? URL, url.pathExtension == "jsonl" else {
                return nil
            }

            let values = try? url.resourceValues(forKeys: Set(keys))
            guard values?.isRegularFile == true else {
                return nil
            }

            if let modified = values?.contentModificationDate, modified < cutoff {
                return nil
            }

            return url
        }
    }
}

extension String {
    var lowercasedModelKey: String {
        lowercased().trimmingCharacters(in: .whitespacesAndNewlines)
    }
}
