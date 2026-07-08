# UsageCheck 크로스플랫폼 재작성 — 설계 문서

- 날짜: 2026-07-08
- 상태: 승인됨 (brainstorming)
- 참고: [bemaru/trafficmonitor-ai-usage-plugin](https://github.com/bemaru/trafficmonitor-ai-usage-plugin)

## 1. 목적

Codex, Claude Code, agy(Antigravity/Gemini)의 사용량(특히 5시간·주간 한도 소진율)을
macOS 메뉴바 / Windows 작업표시줄에 상주 아이콘으로 표시한다. 아이콘 클릭 시 **로그인한 계정별**
현재 5시간·주간 window 사용률 게이지를 팝업으로 보여주며, **한 프로바이더에 여러 계정을 연동**할 수 있다.

### 기존 자산

`Sources/UsageCheckCore/`, `Sources/UsageCheckApp/`에 macOS 전용 Swift 메뉴바 앱이 이미 존재한다.
- 로컬 로그 스캔(Codex/Claude/Gemini) + `chatgpt.com/backend-api/wham/usage`,
  `api.anthropic.com/api/oauth/usage`로 실시간 quota % 조회.
- **단일 계정** 기준. Windows 미지원.

기존 Swift 코드는 참고용으로 보존하고, 로직을 Rust로 1:1 이관한다.

## 2. 아키텍처

**Tauri 앱** — Rust 코어 + 웹(HTML/TS) UI, 단일 코드베이스로 macOS·Windows 동시 지원.

```
┌─────────────────────────────────────────────┐
│  Tauri Shell (main)                          │
│   - TrayIcon (mac 메뉴바 / Win 작업표시줄)     │
│   - 팝업 WebView 윈도우 (무테, 작게)           │
├─────────────────────────────────────────────┤
│  usage-core (Rust lib)                       │
│   - AccountStore  (키체인 연동)               │
│   - OAuthManager  (PKCE, refresh)            │
│   - Fetchers      (Codex/Claude/agy)         │
│   - LogScanners   (로컬 JSONL 집계)           │
│   - Aggregator    (5h/7d/30d window 계산)     │
├─────────────────────────────────────────────┤
│  WebView UI (TS)                             │
│   - 계정 목록 + 게이지 카드                    │
│   - "계정 추가" 흐름                          │
└─────────────────────────────────────────────┘
```

### 구성 요소(각 단위의 책임 · 인터페이스 · 의존성)

- **AccountStore**: 등록된 계정 목록과 자격증명을 OS 키체인(mac Keychain / Windows Credential
  Manager)에 암호화 저장/조회. 인터페이스: `list()`, `add(account)`, `remove(id)`,
  `credentials(id)`. 의존: 키체인 크레이트(`keyring`).
- **OAuthManager**: 프로바이더별 PKCE 인증 개시, 시스템 브라우저 오픈, localhost 콜백 수신, 토큰
  교환·갱신. 인터페이스: `begin_login(provider) -> AuthResult`, `refresh(account)`. 의존: 로컬 HTTP
  리스너, AccountStore.
- **Fetchers**: 계정 자격증명으로 사용량 API 호출, 정규화된 `WindowTotals`/`QuotaUsage` 반환. 의존:
  HTTP 클라이언트. Codex/Claude/agy별 구현.
- **LogScanners**: 로컬 JSONL 로그를 스캔해 토큰 이벤트 집계(폴백/agy 기본). 순수 함수, 파일시스템만
  의존.
- **Aggregator**: 토큰 이벤트를 5h/7d/30d window로 누적. 순수 함수, 의존 없음(유닛테스트 핵심).
- **UI**: `usage-core`가 방출하는 `UsageSnapshot`을 렌더링. Tauri command/event로만 코어와 통신.

각 단위는 잘 정의된 인터페이스로만 통신하며 내부 구현 교체가 소비자를 깨지 않는다.

## 3. 다계정 & 인증

### 계정 추가 흐름

1. 팝업에서 "계정 추가" → 프로바이더 선택(Codex / Claude / agy).
2. **OAuth(PKCE)**: 각 벤더의 공개 client_id로 시스템 브라우저 인증 → localhost 콜백으로 code 수신 →
   token 교환 → 키체인 저장. 계정에 라벨(사용자 지정 별칭) 부여.
3. 만료 시 `refresh_token`으로 자동 갱신. 실패 시 계정 카드에 "재로그인 필요" 배지.

### 폴백 (OAuth 재현이 막히는 프로바이더)

- **config-home 경로 등록**: `CODEX_HOME` / `CLAUDE_CONFIG_DIR` 식으로 CLI가 로그인해 둔 디렉터리를
  등록하면 그 경로의 auth·로그를 읽음.
- **auth 파일 import**: `auth.json` / `.credentials.json`을 직접 지정해 복사·저장.
- **Claude 브라우저-쿠키 웹헬퍼**(참고 프로젝트 방식): 토큰 확보가 곤란할 때 브라우저 프로필 쿠키로
  usage 스냅샷 조회.

## 4. 데이터 수집 (참고 프로젝트 기법 차용)

| 프로바이더 | 1차 소스 | 폴백 소스 |
|---|---|---|
| **Codex** | OAuth 토큰 → `chatgpt.com/backend-api/wham/usage` (5h·주간 %) | 로컬 `~/.codex/sessions/**/*.jsonl`의 `remaining_percent` (`CODEX_HOME` 존중) |
| **Claude** | OAuth 토큰 → `api.anthropic.com/api/oauth/usage` (`five_hour`, `seven_day`) | 브라우저-쿠키 웹헬퍼 스냅샷 |
| **agy/Gemini** | 로컬 로그(`~/.gemini/**/transcript*.jsonl`) 토큰 집계 (best-effort) | — |

- 폴링 주기 기본 60초(설정 가능). 계정별 병렬 조회, 개별 실패 격리.
- **agy 주의**: 공식 quota-% API 부재 가능성이 높아 게이지 대신 토큰 집계로 표시될 수 있음.
  구현 단계에서 API 조사 후 확정.

## 5. 표시 (UI)

- 트레이 아이콘 클릭 → 팝업 윈도우.
- 계정마다 카드: `현재 5시간 %` 게이지 + `주간 7일 %` 게이지 + 리셋 시각.
  참고 프로젝트의 `C5h/C7d`, `X5h/X7d` 표기를 계정별로 확장.
- 프로바이더별 그룹핑, 계정 라벨 표시. "계정 추가/제거" 액션.
- 트레이 아이콘 자체는 요약(예: 최대 소진율 계정의 % 또는 아이콘만) — 상세 설정 가능.

## 6. 에러 처리

- 네트워크 실패·토큰 만료·API 스키마 변경 → 앱은 죽지 않고 마지막 스냅샷 유지, 계정 카드에 상태 배지.
- 토큰 만료 → 자동 refresh, 실패 시 "재로그인 필요".
- 로컬 로그 부재/파싱 실패 → 해당 소스 skip, 다른 소스로 진행.

## 7. 테스트

- **유닛(Rust)**: Aggregator(window 계산), LogScanners(JSONL 파싱), Fetcher 응답 정규화(픽스처 기반).
  기존 Swift 테스트(`Tests/UsageCheckCoreTests`)를 Rust로 이관.
- **수동 스모크**: OAuth 로그인 흐름, 트레이 아이콘/팝업(mac·Windows 각각), 키체인 저장.

## 8. 리스크

- **OAuth 재현**: 각 벤더 공개 client_id/엔드포인트를 조사·재현해야 함. Claude/Codex 가능성 높음,
  agy는 폴백 의존 가능성. → 폴백 경로를 1급 시민으로 설계.
- **agy quota API 부재**: 5h/주간 게이지 대신 로컬 토큰 집계로만 표시될 수 있음.
- **플랫폼 통합**: 트레이/키체인/브라우저 오픈은 mac·Windows 동작 차이 → 수동 검증 필요.

## 9. 범위 밖 (YAGNI)

- 사용량 그래프 히스토리(주간 5시간 블록 타임라인) — 이번 범위 아님(요약 게이지만).
- TrafficMonitor 플러그인 빌드 — 독립 앱으로 대체.
- 모바일/웹 대시보드.
