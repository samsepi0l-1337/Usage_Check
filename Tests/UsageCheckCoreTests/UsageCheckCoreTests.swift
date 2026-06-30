import XCTest
@testable import UsageCheckCore

final class UsageCheckCoreTests: XCTestCase {
    func testCodexScannerReadsSparkTokenEvents() throws {
        let root = try temporaryHome()
        let sessions = root.appendingPathComponent(".codex/sessions/2026/07/01", isDirectory: true)
        try FileManager.default.createDirectory(at: sessions, withIntermediateDirectories: true)

        let now = Date(timeIntervalSince1970: 1_782_876_000)
        let log = sessions.appendingPathComponent("session.jsonl")
        try writeLines([
            #"{"type":"session_meta","payload":{"session_id":"s1"}}"#,
            #"{"type":"turn_context","payload":{"model":"gpt-5.3-codex-spark"}}"#,
            #"{"type":"event_msg","timestamp":"\#(iso(now.addingTimeInterval(-3600)))","payload":{"type":"token_count","session_id":"s1","info":{"last_token_usage":{"input_tokens":100,"cached_input_tokens":10,"output_tokens":20}}}}"#
        ], to: log)

        let paths = UsagePaths(home: root, environment: [:])
        let events = CodexLogScanner(paths: paths).scan(now: now)
        XCTAssertEqual(events.count, 1)
        XCTAssertEqual(events[0].model, "gpt-5.3-codex-spark")
        XCTAssertEqual(events[0].tokens, 130)

        let usage = UsageReader(paths: paths).buildCodexUsage(events: events, quota: nil, now: now)
        XCTAssertEqual(usage.pools.first { $0.id == "all" }?.totals.fiveHours, 130)
        XCTAssertEqual(usage.pools.first { $0.id == "spark" }?.totals.fiveHours, 130)
    }

    func testClaudeScannerDeduplicatesAssistantUsageAndBuildsSonnetPool() throws {
        let root = try temporaryHome()
        let project = root.appendingPathComponent(".claude/projects/example", isDirectory: true)
        try FileManager.default.createDirectory(at: project, withIntermediateDirectories: true)

        let now = Date(timeIntervalSince1970: 1_782_876_000)
        let line = #"{"type":"assistant","timestamp":"\#(iso(now.addingTimeInterval(-120)))","requestId":"r1","message":{"id":"m1","model":"claude-sonnet-4-5","usage":{"input_tokens":10,"cache_creation_input_tokens":2,"cache_read_input_tokens":3,"output_tokens":4}}}"#
        try writeLines([line, line], to: project.appendingPathComponent("conversation.jsonl"))

        let paths = UsagePaths(home: root, environment: [:])
        let events = ClaudeLogScanner(paths: paths).scan(now: now)
        XCTAssertEqual(events.count, 1)
        XCTAssertEqual(events[0].tokens, 19)

        let usage = UsageReader(paths: paths).buildClaudeUsage(events: events, quota: nil, now: now)
        XCTAssertEqual(usage.pools.first { $0.id == "all" }?.totals.fiveHours, 19)
        XCTAssertEqual(usage.pools.first { $0.id == "sonnet" }?.totals.fiveHours, 19)
    }

    func testGeminiScannerReadsTranscriptUsageMetadata() throws {
        let root = try temporaryHome()
        let logs = root.appendingPathComponent(".gemini/antigravity-cli/brain/session/.system_generated/logs", isDirectory: true)
        try FileManager.default.createDirectory(at: logs, withIntermediateDirectories: true)

        let now = Date(timeIntervalSince1970: 1_782_876_000)
        try writeLines([
            #"{"type":"assistant","created_at":"\#(iso(now.addingTimeInterval(-30)))","response":{"model":"gemini-2.5-pro","usageMetadata":{"totalTokenCount":42}}}"#
        ], to: logs.appendingPathComponent("transcript.jsonl"))

        let paths = UsagePaths(home: root, environment: [:])
        let events = GeminiLogScanner(paths: paths).scan(now: now)
        XCTAssertEqual(events.count, 1)
        XCTAssertEqual(events[0].model, "gemini-2.5-pro")
        XCTAssertEqual(events[0].tokens, 42)

        let usage = UsageReader(paths: paths).buildGeminiUsage(events: events, now: now)
        XCTAssertEqual(usage.pools.first { $0.id == "gemini" }?.totals.fiveHours, 42)
        XCTAssertEqual(usage.pools.first { $0.id == "other" }?.totals.fiveHours, 0)
    }

    private func temporaryHome() throws -> URL {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("UsageCheckTests-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }

    private func writeLines(_ lines: [String], to file: URL) throws {
        try FileManager.default.createDirectory(at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
        try lines.joined(separator: "\n").write(to: file, atomically: true, encoding: .utf8)
    }

    private func iso(_ date: Date) -> String {
        ISO8601DateFormatter().string(from: date)
    }
}
