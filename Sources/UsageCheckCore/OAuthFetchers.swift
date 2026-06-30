import Foundation

struct CodexQuotaResult: Sendable {
    var plan: String?
    var fiveHour: QuotaUsage?
    var week: QuotaUsage?
    var fetchedAt: Date
}

struct ClaudeQuotaResult: Sendable {
    var plan: String?
    var fiveHour: QuotaUsage?
    var week: QuotaUsage?
    var fetchedAt: Date
}

struct OAuthFetchers: Sendable {
    var paths: UsagePaths
    var session: URLSession

    init(paths: UsagePaths = UsagePaths(), session: URLSession = .shared) {
        self.paths = paths
        self.session = session
    }

    func fetchCodex() async -> CodexQuotaResult? {
        guard let credentials = readCodexCredentials() else {
            return nil
        }

        guard let url = URL(string: "https://chatgpt.com/backend-api/wham/usage") else {
            return nil
        }

        var request = URLRequest(url: url, timeoutInterval: 10)
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        request.setValue("UsageCheck", forHTTPHeaderField: "User-Agent")
        request.setValue("Bearer \(credentials.accessToken)", forHTTPHeaderField: "Authorization")
        if let accountID = credentials.accountID {
            request.setValue(accountID, forHTTPHeaderField: "ChatGPT-Account-Id")
        }

        do {
            let (data, response) = try await session.data(for: request)
            guard (response as? HTTPURLResponse)?.statusCode == 200,
                  let root = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
                return nil
            }

            let rateLimit = JSONValue.dictionary(root["rate_limit"])
            let primary = JSONValue.dictionary(rateLimit?["primary_window"])
            let secondary = JSONValue.dictionary(rateLimit?["secondary_window"])

            return CodexQuotaResult(
                plan: JSONValue.string(root["plan_type"]),
                fiveHour: quotaFromCodexWindow(primary),
                week: quotaFromCodexWindow(secondary),
                fetchedAt: Date()
            )
        } catch {
            return nil
        }
    }

    func fetchClaude() async -> ClaudeQuotaResult? {
        guard let credentials = readClaudeCredentials() else {
            return nil
        }

        guard let url = URL(string: "https://api.anthropic.com/api/oauth/usage") else {
            return nil
        }

        var request = URLRequest(url: url, timeoutInterval: 20)
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        request.setValue("oauth-2025-04-20", forHTTPHeaderField: "anthropic-beta")
        request.setValue("claude-code/2.1.70", forHTTPHeaderField: "User-Agent")
        request.setValue("Bearer \(credentials.accessToken)", forHTTPHeaderField: "Authorization")

        do {
            let (data, response) = try await session.data(for: request)
            guard (response as? HTTPURLResponse)?.statusCode == 200,
                  let root = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
                return nil
            }

            return ClaudeQuotaResult(
                plan: credentials.subscriptionType,
                fiveHour: quotaFromClaudeWindow(JSONValue.dictionary(root["five_hour"])),
                week: quotaFromClaudeWindow(JSONValue.dictionary(root["seven_day"])),
                fetchedAt: Date()
            )
        } catch {
            return nil
        }
    }

    private func quotaFromCodexWindow(_ dictionary: [String: Any]?) -> QuotaUsage? {
        guard let dictionary, let percent = JSONValue.double(dictionary["used_percent"]) else {
            return nil
        }

        let resetsAt = JSONValue.double(dictionary["reset_at"]).map { Date(timeIntervalSince1970: $0) }
        let windowSeconds = JSONValue.int64(dictionary["limit_window_seconds"])
        return QuotaUsage(
            percent: percent,
            resetsAt: resetsAt,
            windowSeconds: windowSeconds > 0 ? Int(windowSeconds) : nil
        )
    }

    private func quotaFromClaudeWindow(_ dictionary: [String: Any]?) -> QuotaUsage? {
        guard let dictionary, let percent = JSONValue.double(dictionary["utilization"]) else {
            return nil
        }

        return QuotaUsage(
            percent: percent,
            resetsAt: TimestampParser.parse(dictionary["resets_at"]),
            windowSeconds: nil
        )
    }

    private func readCodexCredentials() -> CodexCredentials? {
        guard let data = try? Data(contentsOf: paths.codexAuthFile),
              let root = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            return nil
        }

        if let tokens = JSONValue.dictionary(root["tokens"]),
           let accessToken = JSONValue.string(tokens["access_token"]),
           !accessToken.isEmpty {
            return CodexCredentials(
                accessToken: accessToken,
                accountID: JSONValue.string(tokens["account_id"])
            )
        }

        if let accessToken = JSONValue.string(root["OPENAI_API_KEY"]), !accessToken.isEmpty {
            return CodexCredentials(accessToken: accessToken, accountID: nil)
        }

        return nil
    }

    private func readClaudeCredentials() -> ClaudeCredentials? {
        for file in paths.claudeCredentialFiles {
            guard let data = try? Data(contentsOf: file),
                  let root = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                  let oauth = JSONValue.dictionary(root["claudeAiOauth"]),
                  let accessToken = JSONValue.string(oauth["accessToken"]),
                  !accessToken.isEmpty else {
                continue
            }

            if let expiresAt = JSONValue.double(oauth["expiresAt"]) {
                let seconds = expiresAt > 10_000_000_000 ? expiresAt / 1000 : expiresAt
                if Date(timeIntervalSince1970: seconds) < Date() {
                    continue
                }
            }

            return ClaudeCredentials(
                accessToken: accessToken,
                subscriptionType: JSONValue.string(oauth["subscriptionType"])
            )
        }

        return nil
    }
}

private struct CodexCredentials {
    var accessToken: String
    var accountID: String?
}

private struct ClaudeCredentials {
    var accessToken: String
    var subscriptionType: String?
}
