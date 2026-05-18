# Phase 5 Handoff — Auth Rate Limiting

**Date:** 2026-05-18  
**Branch:** `main` (pushed to `origin/main`, commit range `59abe48`–`df1fa9a`)  
**Status:** **COMPLETE**  
**Tests:** 125 API / 29 core — all green (154 total)

---

## What Shipped

Per-route IP-based rate limiting on `/auth/login` (10/min), `/auth/register` (5/min), and `/auth/refresh` (30/min) using `tower_governor 0.8` with `PeerIpKeyExtractor`. Three integration tests assert the 429 path without sleeps via a `tight_app` helper (burst=1).

| Commit | Change |
|--------|--------|
| `84b59fe` | Design spec (initial): per-route GovernorLayer approach, ConnectInfo fix, tight_app strategy |
| `9d55ff7` | Spec fix: per_second(N) footnote, tight_app path prefix, test example made explicit |
| `73bf948` | Spec fix: r1 asserts 422 (deterministic), Arc-clone note, Acceptance Criteria section added |
| `7a8d987` | Spec fix: off-by-one in acceptance criteria (11th/6th/31st request rejected) |
| `294893c` | Implementation plan: 4 tasks / 18 steps, TDD red→green, all code included |
| `faca1f9` | deps(api): add tower_governor 0.8; promote tower to prod dep |
| `4be2b71` | test(auth): add failing 429 rate-limit tests (red) |
| `e35e942` | feat(auth): per-route rate limiting via tower_governor |
| `0b959d3` | style(auth): fix clippy warnings (pre-existing unused imports in monitor/notification_channel) |
| `df1fa9a` | fix(api): inject ConnectInfo for PeerIpKeyExtractor in production server (main.rs) |

---

## Design Decisions (summary)

- **Approach:** Approach A — per-route `GovernorLayer` via `MethodRouter::layer()` inside `auth::router()`. `lib.rs` untouched.
- **Limits:** login 10/min (burst 10, per_second 6), register 5/min (burst 5, per_second 12), refresh 30/min (burst 30, per_second 2)
- **Key extractor:** `PeerIpKeyExtractor` — reads `ConnectInfo<SocketAddr>`. No trusted-proxy header support (out of scope).
- **Tests:** `tight_app` helper with burst=1 and per_second=3600; `Router::clone()` shares `Arc<GovernorConfig>` state. r1 asserts 422 (extractor rejects `{}`), r2 asserts 429.
- **ConnectInfo fix:** `post_json` test helper in `auth/mod.rs` must inject `.extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))))` so `PeerIpKeyExtractor` finds the extension in existing tests.

Full spec: `docs/superpowers/specs/2026-05-18-auth-rate-limiting-design.md`

---

## Implementation Plan

`docs/superpowers/plans/2026-05-18-auth-rate-limiting.md`

4 tasks, 18 steps:

| Task | What |
|------|------|
| Task 1 | Resolve axum-0.8-compatible `tower_governor` version; update `Cargo.toml`; `cargo check` |
| Task 2 | Write failing 429 tests + `tight_app` stub (red phase) |
| Task 3 | Wire `GovernorLayer` per route + complete `tight_app` + fix `post_json` (green phase) |
| Task 4 | Acceptance check: 154 tests, `cargo clippy`, `/health` unaffected |

**First step:** Run `cargo search tower_governor` and verify the latest release targets axum 0.8 / tower 0.5 (the Phase 4 handoff cited `"0.4"` which targets axum 0.7 — do not use it).

---

## Architecture State (post-Phase 5)

`signalnode-api/src/auth/`:

```
token.rs    — Claims { sub, exp, kind, jti: Option<String> }
              encode_access_token → Result<String, _>
              encode_refresh_token → Result<(String, Uuid, DateTime<Utc>), _>
              decode_access_token  — enforces kind = "access"
              decode_refresh_token — enforces kind = "refresh" AND jti.is_some()

mod.rs      — router(): POST /register (GovernorLayer 5/min, burst 5)
                        POST /login    (GovernorLayer 10/min, burst 10)
                        POST /refresh  (GovernorLayer 30/min, burst 30)
              login:   Argon2id verify → encode tokens → INSERT jti → 201
              refresh: decode JWT → DELETE old jti (401 if 0 rows) → INSERT new jti → AuthResponse
```

`main.rs`: `axum::serve` uses `into_make_service_with_connect_info::<SocketAddr>()` so `PeerIpKeyExtractor` receives the peer IP in production.

**429 test coverage:** `login_rate_limited_returns_429`, `register_rate_limited_returns_429`, `refresh_rate_limited_returns_429` — each fires two requests through a `tight_app` (burst=1, per_second=3600); r1→422 (extractor rejects `{}`), r2→429 (burst exhausted).

`refresh_tokens` table (migration `20260518000012`):

```sql
jti        UUID PRIMARY KEY
user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE
expires_at TIMESTAMPTZ NOT NULL
created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
INDEX refresh_tokens_user_id_idx ON (user_id)
```

---

## Known Technical Debt (updated post-Phase 5)

| Item | Severity | Notes |
|---|---|---|
| ~~No rate limiting on auth endpoints~~ | ~~High~~ | **Done — Phase 5** |
| No account lockout after failed logins | **High** | **Recommended next (Phase 6).** Requires HITL review before designing. Complements rate limiting; rate limits slow brute-force, lockout stops it. |
| `refresh_tokens` grows indefinitely | Medium | Expired rows never purged. Needs scheduled `DELETE WHERE expires_at < NOW()`. |
| No structured error response bodies | Medium | Bare status codes. Clients can't distinguish error types. |
| Incident logic duplicated between `checker.rs` and `signalnode-api/src/check_result/mod.rs` | Medium | Extract to `signalnode-shared` crate. |
| `check_membership` / `check_owner` duplicated across three API modules | Low | Extraction deferred. |

---

## Test Counts (post-Phase 5 implementation)

| Crate | Tests | Status |
|-------|-------|--------|
| signalnode-api | 125 | all pass (includes 3 new 429 tests) |
| signalnode-core | 29 | all pass |
| **Total** | **154** | **all green** |

**Test commands:**
```bash
DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-api/Cargo.toml

DATABASE_URL=postgres://signalnode:signalnode@localhost:5432/signalnode \
  cargo test --manifest-path signalnode-core/Cargo.toml
```

---

## Project Docs

- Domain glossary: `CONTEXT.md`
- Architecture decisions: `docs/adr/`
- Phase 1 PRD: `docs/prd-phase1.md`
- Phase 3 handoff: `docs/phase3-handoff.md`
- Phase 4 handoff: `docs/phase4-handoff.md`
- Phase 5 spec: `docs/superpowers/specs/2026-05-18-auth-rate-limiting-design.md`
- Phase 5 plan: `docs/superpowers/plans/2026-05-18-auth-rate-limiting.md`
