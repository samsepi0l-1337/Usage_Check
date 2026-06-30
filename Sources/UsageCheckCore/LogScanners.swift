import Foundation

protocol UsageLogScanning {
    func scan(now: Date) -> [ModelTokenEvent]
}

struct CodexLogScanner: UsageLogScanning {
    var paths: UsagePaths

    init(paths: UsagePaths = UsagePaths()) {
        self.paths = paths
    }

    func scan(now: Date) -> [ModelTokenEvent] {
        let cutoff = now.addingTimeInterval(-(UsageWindow.month.duration + 24 * 60 * 60))
        let files = FileEnumerator.jsonlFiles(roots: paths.codexSessionRoots, modifiedSince: cutoff)
        var events: [ModelTokenEvent] = []

        for file in files {
            var activeSessionID: String?
            var currentModel: String?
            var previousTotalsBySession: [String: TokenTotals] = [:]

            LineReader.readLines(from: file) { line in
                guard line.contains("\"type\"") else {
                    return
                }

                guard let root = JSONValue.dictionary(JSONValue.parse(line)),
                      let type = JSONValue.string(root["type"]) else {
                    return
                }

                if type == "session_meta" {
                    activeSessionID = sessionID(from: root) ?? activeSessionID
                    return
                }

                if type == "turn_context" {
                    if let payload = JSONValue.dictionary(root["payload"]) {
                        if let model = JSONValue.string(payload["model"]) {
                            currentModel = model
                        } else if let info = JSONValue.dictionary(payload["info"]),
                                  let model = JSONValue.string(info["model"]) {
                            currentModel = model
                        }
                    }
                    return
                }

                guard type == "event_msg",
                      let timestamp = TimestampParser.find(in: root),
                      timestamp >= cutoff,
                      let payload = JSONValue.dictionary(root["payload"]),
                      JSONValue.string(payload["type"]) == "token_count" else {
                    return
                }

                let session = sessionID(from: root) ?? activeSessionID ?? file.deletingPathExtension().lastPathComponent
                let info = JSONValue.dictionary(payload["info"]) ?? payload
                let model = JSONValue.string(info["model"])
                    ?? JSONValue.string(info["model_name"])
                    ?? currentModel
                    ?? "gpt-5"

                let tokens = extractCodexTokens(
                    from: info,
                    previousTotalsBySession: &previousTotalsBySession,
                    sessionID: session
                )

                guard tokens.total > 0 else {
                    return
                }

                events.append(
                    ModelTokenEvent(
                        timestamp: timestamp,
                        model: model,
                        tokens: tokens.total,
                        dedupeKey: "codex:\(session):\(timestamp.timeIntervalSince1970):\(tokens.total)"
                    )
                )
            }
        }

        return events
    }

    private func extractCodexTokens(
        from info: [String: Any],
        previousTotalsBySession: inout [String: TokenTotals],
        sessionID: String
    ) -> TokenTotals {
        if let last = JSONValue.dictionary(info["last_token_usage"]) {
            return TokenTotals(
                input: JSONValue.int64(last["input_tokens"]),
                cached: JSONValue.int64(last["cached_input_tokens"]) + JSONValue.int64(last["cache_read_input_tokens"]),
                output: JSONValue.int64(last["output_tokens"])
            )
        }

        if let total = JSONValue.dictionary(info["total_token_usage"]) {
            let totals = TokenTotals(
                input: JSONValue.int64(total["input_tokens"]),
                cached: JSONValue.int64(total["cached_input_tokens"]) + JSONValue.int64(total["cache_read_input_tokens"]),
                output: JSONValue.int64(total["output_tokens"])
            )

            let delta: TokenTotals
            if let previous = previousTotalsBySession[sessionID] {
                delta = totals.delta(from: previous)
            } else {
                delta = totals
            }

            previousTotalsBySession[sessionID] = totals
            return delta
        }

        return .zero
    }

    private func sessionID(from root: [String: Any]) -> String? {
        if let payload = JSONValue.dictionary(root["payload"]),
           let sessionID = JSONValue.string(payload["session_id"]),
           !sessionID.isEmpty {
            return sessionID
        }

        if let sessionID = JSONValue.string(root["session_id"]), !sessionID.isEmpty {
            return sessionID
        }

        return nil
    }
}

struct ClaudeLogScanner: UsageLogScanning {
    var paths: UsagePaths

    init(paths: UsagePaths = UsagePaths()) {
        self.paths = paths
    }

    func scan(now: Date) -> [ModelTokenEvent] {
        let cutoff = now.addingTimeInterval(-(UsageWindow.month.duration + 24 * 60 * 60))
        let files = FileEnumerator.jsonlFiles(roots: paths.claudeProjectRoots, modifiedSince: cutoff)
        var events: [ModelTokenEvent] = []
        var seenKeys = Set<String>()

        for file in files {
            LineReader.readLines(from: file) { line in
                guard line.contains("\"type\""),
                      line.contains("\"usage\""),
                      let root = JSONValue.dictionary(JSONValue.parse(line)),
                      JSONValue.string(root["type"]) == "assistant",
                      let timestamp = TimestampParser.find(in: root),
                      timestamp >= cutoff,
                      let message = JSONValue.dictionary(root["message"]),
                      let usage = JSONValue.dictionary(message["usage"]) else {
                    return
                }

                let messageID = JSONValue.string(message["id"])
                let requestID = JSONValue.string(root["requestId"])
                if let messageID, let requestID {
                    let key = "\(messageID):\(requestID)"
                    guard seenKeys.insert(key).inserted else {
                        return
                    }
                }

                let tokens =
                    JSONValue.int64(usage["input_tokens"])
                    + JSONValue.int64(usage["cache_creation_input_tokens"])
                    + JSONValue.int64(usage["cache_read_input_tokens"])
                    + JSONValue.int64(usage["output_tokens"])

                guard tokens > 0 else {
                    return
                }

                events.append(
                    ModelTokenEvent(
                        timestamp: timestamp,
                        model: JSONValue.string(message["model"]) ?? "claude",
                        tokens: tokens,
                        dedupeKey: messageID.map { "claude:\($0):\(requestID ?? "")" }
                    )
                )
            }
        }

        return events
    }
}

struct GeminiLogScanner: UsageLogScanning {
    var paths: UsagePaths

    init(paths: UsagePaths = UsagePaths()) {
        self.paths = paths
    }

    func scan(now: Date) -> [ModelTokenEvent] {
        let cutoff = now.addingTimeInterval(-(UsageWindow.month.duration + 24 * 60 * 60))
        let files = FileEnumerator
            .jsonlFiles(roots: paths.geminiLogRoots, modifiedSince: cutoff)
            .filter { $0.lastPathComponent.hasPrefix("transcript") || $0.path.contains("/logs/") }

        var events: [ModelTokenEvent] = []
        var seenKeys = Set<String>()

        for file in files {
            LineReader.readLines(from: file) { line in
                guard line.contains("{") else {
                    return
                }

                guard let root = JSONValue.dictionary(JSONValue.parse(line)),
                      let timestamp = TimestampParser.find(in: root),
                      timestamp >= cutoff else {
                    return
                }

                let tokens = GeminiLogScanner.findTokenTotal(in: root)
                guard tokens > 0 else {
                    return
                }

                let model = GeminiLogScanner.findModel(in: root) ?? "gemini"
                let key = "gemini:\(file.path):\(timestamp.timeIntervalSince1970):\(model):\(tokens)"
                guard seenKeys.insert(key).inserted else {
                    return
                }

                events.append(
                    ModelTokenEvent(
                        timestamp: timestamp,
                        model: model,
                        tokens: tokens,
                        dedupeKey: key
                    )
                )
            }
        }

        return events
    }

    static func findModel(in value: Any) -> String? {
        if let dictionary = value as? [String: Any] {
            for key in ["model", "model_name", "modelName", "model_id", "modelId"] {
                if let model = JSONValue.string(dictionary[key]), isPlausibleModel(model) {
                    return model
                }
            }

            for child in dictionary.values {
                if let model = findModel(in: child) {
                    return model
                }
            }
        } else if let array = value as? [Any] {
            for child in array {
                if let model = findModel(in: child) {
                    return model
                }
            }
        }

        return nil
    }

    static func findTokenTotal(in value: Any) -> Int64 {
        if let dictionary = value as? [String: Any] {
            if let usage = dictionary["usageMetadata"] ?? dictionary["usage_metadata"] ?? dictionary["usage"],
               let total = usageTotal(from: usage) {
                return total
            }

            if let direct = directTokenTotal(from: dictionary), direct > 0 {
                return direct
            }

            var best: Int64 = 0
            for child in dictionary.values {
                best = max(best, findTokenTotal(in: child))
            }
            return best
        }

        if let array = value as? [Any] {
            return array.map(findTokenTotal(in:)).max() ?? 0
        }

        return 0
    }

    private static func usageTotal(from value: Any) -> Int64? {
        guard let dictionary = value as? [String: Any] else {
            return nil
        }

            let explicitTotal = firstInt(in: dictionary, keys: [
                "totalTokenCount",
                "total_token_count",
                "totalTokens",
                "total_tokens"
            ])
            if explicitTotal > 0 {
                return explicitTotal
            }

        let prompt = firstInt(in: dictionary, keys: [
            "promptTokenCount",
            "prompt_token_count",
            "inputTokenCount",
            "input_tokens"
        ])

        let candidates = firstInt(in: dictionary, keys: [
            "candidatesTokenCount",
            "candidates_token_count",
            "completionTokenCount",
            "completion_tokens",
            "outputTokenCount",
            "output_tokens"
        ])

        let cached = firstInt(in: dictionary, keys: [
            "cachedContentTokenCount",
            "cached_content_token_count",
            "cache_read_input_tokens"
        ])

        let thoughts = firstInt(in: dictionary, keys: [
            "thoughtsTokenCount",
            "thoughts_token_count"
        ])

        let total = prompt + candidates + cached + thoughts
        return total > 0 ? total : nil
    }

    private static func directTokenTotal(from dictionary: [String: Any]) -> Int64? {
        let interestingKeys = dictionary.keys.filter { key in
            let lower = key.lowercased()
            return lower.contains("token") && !lower.contains("limit")
        }

        guard !interestingKeys.isEmpty else {
            return nil
        }

        if let totalKey = interestingKeys.first(where: { $0.lowercased().contains("total") }) {
            let total = JSONValue.int64(dictionary[totalKey])
            if total > 0 {
                return total
            }
        }

        let sum = interestingKeys
            .filter { !$0.lowercased().contains("total") }
            .map { JSONValue.int64(dictionary[$0]) }
            .reduce(0, +)

        return sum > 0 ? sum : nil
    }

    private static func firstInt(in dictionary: [String: Any], keys: [String]) -> Int64 {
        for key in keys {
            let value = JSONValue.int64(dictionary[key])
            if value > 0 {
                return value
            }
        }

        return 0
    }

    private static func isPlausibleModel(_ value: String) -> Bool {
        let lower = value.lowercased()
        return lower.contains("gemini")
            || lower.contains("claude")
            || lower.contains("gpt")
            || lower.contains("codex")
            || lower.contains("openai")
    }
}

private struct TokenTotals: Equatable {
    static let zero = TokenTotals(input: 0, cached: 0, output: 0)

    var input: Int64
    var cached: Int64
    var output: Int64

    var total: Int64 {
        input + cached + output
    }

    func delta(from previous: TokenTotals) -> TokenTotals {
        TokenTotals(
            input: max(0, input - previous.input),
            cached: max(0, cached - previous.cached),
            output: max(0, output - previous.output)
        )
    }
}
