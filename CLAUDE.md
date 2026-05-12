# CLAUDE.md

## Purpose

This repository contains the source code and architecture for SignalNode.

SignalNode is a modern infrastructure-focused monitoring platform designed to grow beyond WordPress into a scalable monitoring and SaaS ecosystem.

The repository includes:
- Rust monitoring engine
- API/backend services
- optional node agent
- future dashboard/web frontend
- WordPress integration
- future Kubernetes and SaaS capabilities

Claude should treat this repository as:
- a long-term platform engineering project
- infrastructure-oriented software
- API-first architecture
- container-first architecture
- future Kubernetes-ready architecture

Read `CONTEXT.md` before making architectural assumptions.

---

# Engineering Philosophy

## Design Priorities

Prioritize:
1. clarity
2. maintainability
3. modularity
4. scalability
5. observability
6. portability

Avoid:
- premature optimization
- overengineering Phase 1
- WordPress-centric assumptions
- tightly coupled services
- monolithic architecture decisions

---

# Development Rules

## Before Making Changes

Claude should:
1. inspect existing files first
2. explain the proposed approach
3. make the smallest safe change possible
4. preserve repo structure consistency
5. avoid unnecessary rewrites
6. avoid destructive refactors unless requested

---

# Repository Structure

```text
SignalNode
├── signalnode-core
├── signalnode-api
├── signalnode-agent
├── signalnode-web
├── signalnode-wp-plugin
├── docs
└── .github
```

## Expected Responsibilities

### signalnode-core

Rust monitoring engine.

Responsibilities:
- monitor execution
- uptime checks
- SSL checks
- API checks
- scheduling
- concurrency
- async networking
- health evaluation
- future worker logic

Avoid:
- frontend logic
- WordPress-specific logic
- SaaS billing logic

---

### signalnode-api

Primary backend/API service.

Responsibilities:
- REST API
- authentication
- monitor management
- alert management
- future SaaS boundaries
- dashboard/backend integration

Prefer:
- clean API boundaries
- OpenAPI-compatible design
- modular services

---

### signalnode-agent

Optional lightweight agent.

Responsibilities:
- future node/server monitoring
- local metric collection
- secure communication with API
- future Kubernetes/node integrations

The agent should remain lightweight and container-friendly.

---

### signalnode-web

Future frontend/dashboard.

Responsibilities:
- dashboard UI
- monitor management UI
- status visualization
- future SaaS admin interfaces

Avoid tightly coupling frontend and monitoring engine logic.

---

### signalnode-wp-plugin

WordPress integration layer.

Responsibilities:
- connect WordPress sites to SignalNode
- display monitoring information
- configure monitors from WordPress
- act as integration layer only

Avoid:
- implementing core monitoring logic in PHP
- duplicating monitoring engine behavior

SignalNode is not a WordPress-only product.

---

# Rust Guidance

Rust is preferred because of:
- async performance
- concurrency
- low memory usage
- portability
- good container deployment characteristics

Preferred ecosystem direction:
- `tokio`
- `axum`
- `reqwest`
- `serde`
- `sqlx`
- `tracing`

Claude should prefer:
- idiomatic Rust
- async-first design
- modular crates
- clean error handling
- structured logging

Avoid:
- unnecessary unsafe code
- giant files/modules
- blocking async operations

---

# API Design Rules

API design should:
- remain REST-friendly initially
- support future SaaS growth
- support future multi-tenancy
- maintain stable naming conventions

Prefer:
- explicit naming
- versioned APIs later
- OpenAPI documentation
- JSON-first design

---

# Infrastructure Expectations

SignalNode is expected to support:
- Docker
- Docker Compose
- future Kubernetes deployment
- GitHub Actions CI/CD
- future GitOps deployment

Claude should:
- prefer container-friendly architecture
- avoid assumptions tied to one hosting provider
- keep deployment portability in mind

---

# Documentation Rules

Documentation is important.

When generating docs:
- keep explanations practical
- avoid excessive marketing language
- explain architecture decisions clearly
- document tradeoffs
- keep examples realistic

Prefer:
- markdown
- diagrams when useful
- step-by-step implementation notes
- ADRs for major architecture decisions

---

# ADR Rules

Create ADRs only when:
- a decision is difficult to reverse
- a tradeoff exists
- future contributors would need context
- architecture direction changes significantly

Do not create ADRs for trivial implementation details.

---

# GitHub Actions / CI Expectations

CI/CD should eventually include:
- Rust formatting checks
- clippy
- tests
- container builds
- security scanning
- future deployment pipelines

Prefer:
- simple pipelines first
- incremental complexity
- reusable workflows

---

# Security Expectations

Never:
- commit secrets
- commit API keys
- commit kubeconfigs
- commit production credentials

Prefer:
- environment variables
- secrets management
- least privilege
- secure defaults

---

# Claude Workflow Expectations

Claude should:
- ask clarifying questions when architecture is unclear
- challenge vague terminology
- recommend cleaner boundaries when needed
- help refine domain language
- help improve repo organization over time

When using grill-with-docs:
- ask one question at a time
- challenge assumptions
- propose alternatives
- update terminology when clarified

---

# Long-Term Direction

SignalNode may eventually evolve into:
- SaaS monitoring platform
- Kubernetes monitoring platform
- distributed monitoring system
- multi-region monitoring platform
- observability platform

Early architecture should keep this direction in mind without overengineering Phase 1.
