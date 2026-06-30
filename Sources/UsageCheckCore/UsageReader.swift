import Foundation

public struct UsageReader {
    private var paths: UsagePaths
    private var oauth: OAuthFetchers

    public init() {
        let paths = UsagePaths()
        self.paths = paths
        self.oauth = OAuthFetchers(paths: paths)
    }

    init(paths: UsagePaths, oauth: OAuthFetchers? = nil) {
        self.paths = paths
        self.oauth = oauth ?? OAuthFetchers(paths: paths)
    }

    public func readSnapshot() async -> UsageSnapshot {
        let now = Date()

        let codexEvents = CodexLogScanner(paths: paths).scan(now: now)
        let claudeEvents = ClaudeLogScanner(paths: paths).scan(now: now)
        let geminiEvents = GeminiLogScanner(paths: paths).scan(now: now)

        let codexQuota = await oauth.fetchCodex()
        let claudeQuota = await oauth.fetchClaude()

        let providers = [
            buildCodexUsage(events: codexEvents, quota: codexQuota, now: now),
            buildClaudeUsage(events: claudeEvents, quota: claudeQuota, now: now),
            buildGeminiUsage(events: geminiEvents, now: now)
        ]

        return UsageSnapshot(capturedAt: now, providers: providers)
    }

    func buildCodexUsage(events: [ModelTokenEvent], quota: CodexQuotaResult?, now: Date) -> ProviderUsage {
        let all = totals(for: events, now: now)
        let spark = totals(
            for: events.filter { event in
                event.model.lowercasedModelKey.contains("gpt-5.3-codex-spark")
                    || event.model.lowercasedModelKey.contains("codex-spark")
            },
            now: now
        )

        let source = sourceSummary(eventCount: events.count, hasAPI: quota != nil)
        return ProviderUsage(
            provider: .codex,
            pools: [
                PoolUsage(
                    id: "all",
                    displayName: "전체 풀",
                    totals: all,
                    fiveHourQuota: quota?.fiveHour,
                    weekQuota: quota?.week,
                    note: quota == nil ? "API 없음: 로컬 로그 토큰 합계" : "API 사용률 + 로컬 30일 토큰"
                ),
                PoolUsage(
                    id: "spark",
                    displayName: "gpt-5.3-codex-spark 풀",
                    totals: spark,
                    note: "로컬 로그 기준"
                )
            ],
            sourceSummary: source
        )
    }

    func buildClaudeUsage(events: [ModelTokenEvent], quota: ClaudeQuotaResult?, now: Date) -> ProviderUsage {
        let all = totals(for: events, now: now)
        let sonnet = totals(
            for: events.filter { event in
                event.model.lowercasedModelKey.contains("sonnet")
            },
            now: now
        )

        return ProviderUsage(
            provider: .claude,
            pools: [
                PoolUsage(
                    id: "all",
                    displayName: "전체 풀",
                    totals: all,
                    fiveHourQuota: quota?.fiveHour,
                    weekQuota: quota?.week,
                    note: quota == nil ? "API 없음: 로컬 로그 토큰 합계" : "API 사용률 + 로컬 30일 토큰"
                ),
                PoolUsage(
                    id: "sonnet",
                    displayName: "Sonnet 풀",
                    totals: sonnet,
                    note: "로컬 로그 기준"
                )
            ],
            sourceSummary: sourceSummary(eventCount: events.count, hasAPI: quota != nil)
        )
    }

    func buildGeminiUsage(events: [ModelTokenEvent], now: Date) -> ProviderUsage {
        let gemini = totals(
            for: events.filter { event in
                event.model.lowercasedModelKey.contains("gemini")
            },
            now: now
        )

        let other = totals(
            for: events.filter { event in
                !event.model.lowercasedModelKey.contains("gemini")
            },
            now: now
        )

        return ProviderUsage(
            provider: .gemini,
            pools: [
                PoolUsage(
                    id: "gemini",
                    displayName: "Gemini 모델 풀",
                    totals: gemini,
                    note: "로컬 로그 기준"
                ),
                PoolUsage(
                    id: "other",
                    displayName: "기타 모델 풀",
                    totals: other,
                    note: "로컬 로그 기준"
                )
            ],
            sourceSummary: sourceSummary(eventCount: events.count, hasAPI: false)
        )
    }

    private func totals(for events: [ModelTokenEvent], now: Date) -> WindowTotals {
        var totals = WindowTotals()
        for event in events {
            totals.add(tokens: event.tokens, at: event.timestamp, now: now)
        }
        return totals
    }

    private func sourceSummary(eventCount: Int, hasAPI: Bool) -> String {
        switch (eventCount > 0, hasAPI) {
        case (true, true):
            return "API + local logs"
        case (false, true):
            return "API only"
        case (true, false):
            return "Local logs"
        case (false, false):
            return "No usage data"
        }
    }
}
