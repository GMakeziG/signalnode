# Phase 5 Handoff — Auth Rate Limiting Design

**Date:** 2026-05-18  
**Branch:** `main` (pushed to `origin/main`, commit range `59abe48`–`294893c`)  
**Tests:** 122 API / 29 core — all green (151 total)

---

## What Shipped

Design spec and implementation plan for per-route IP-based rate limiting on the three auth endpoints using `tower_governor`. No production code written yet.

| Commit | Change |
|--------|--------|
| `84b59fe` | Design spec (initial): per-route GovernorLayer approach, ConnectInfo fix, tight_app strategy |
| `9d55ff7` | Spec fix: per_second(N) footnote, tight_app path prefix, test example made explicit |
| `73bf948` | Spec fix: r1 asserts 422 (deterministic), Arc-clone note, Acceptance Criteria section added |
| `7a8d987` | Spec fix: off-by-one in acceptance criteria (11th/6th/31st request rejected) |
| `294893c` | Implementation plan: 4 tasks / 18 steps, TDD red→green, all code included |

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

## Architecture State (post-Phase 4, pre-Phase 5)

`signalnode-api/src/auth/`:

```
token.rs    — Claims { sub, exp, kind, jti: Option<String> }
              encode_access_token → Result<String, _>
              encode_refresh_token → Result<(String, Uuid, DateTime<Utc>), _>
              decode_access_token  — enforces kind = "access"
              decode_refresh_token — enforces kind = "refresh" AND jti.is_some()

mod.rs      — router(): POST /register, /login, /refresh  ← GovernorLayer goes here
              login:   Argon2id verify → encode tokens → INSERT jti → 201
              refresh: decode JWT → DELETE old jti (401 if 0 rows) → INSERT new jti → AuthResponse
```

`refresh_tokens` table (migration `20260518000012`):

```sql
jti        UUID PRIMARY KEY
user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE
expires_at TIMESTAMPTZ NOT NULL
created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
INDEX refresh_tokens_user_id_idx ON (user_id)
```

---

## Known Technical Debt (updated)

| Item | Severity | Notes |
|---|---|---|
| No rate limiting on auth endpoints | **High** | **Phase 5 — implementation plan ready** |
| `refresh_tokens` grows indefinitely | Medium | Expired rows never purged. Needs scheduled `DELETE WHERE expires_at < NOW()`. |
| No structured error response bodies | Medium | Bare status codes. Clients can't distinguish error types. |
| No account lockout after failed logins | Medium | Requires HITL review before implementing. |
| Incident logic duplicated between `checker.rs` and `signalnode-api/src/check_result/mod.rs` | Medium | Extract to `signalnode-shared` crate (Phase 6+). |
| `check_membership` / `check_owner` duplicated across three API modules | Low | Extraction deferred. |
| Unused imports in `monitor/mod.rs` and `notification_channel/mod.rs` | Low | Pre-existing warnings. |

---

## Test Counts (as of 2026-05-18 Phase 5 design)

| Crate | Tests | Status |
|-------|-------|--------|
| signalnode-api | 122 | all pass |
| signalnode-core | 29 | all pass |

After Phase 5 implementation: signalnode-api should reach **125 tests** (3 new 429 tests).

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
