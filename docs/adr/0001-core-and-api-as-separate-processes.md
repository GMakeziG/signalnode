# signalnode-core and signalnode-api run as separate processes

`signalnode-core` (the Rust monitoring engine) and `signalnode-api` (the HTTP backend) are deployed as independent processes that share a single database. The engine polls for active Monitors, executes checks, and writes CheckResults directly to the database; the API reads and writes the same database to serve the REST surface.

We chose this over embedding both in one binary because it matches the existing crate boundaries, allows each process to be scaled and restarted independently, and keeps the check execution path free of HTTP request lifecycle concerns. We chose it over a message queue approach because the shared database is sufficient coordination for Phase 1 and avoids the operational overhead of a broker.

## Considered Options

- **Single binary** — simpler deployment, but conflates HTTP request handling with long-running scheduler logic and makes horizontal scaling of just the check engine impossible.
- **Queue-based (core consumes jobs from a queue)** — maximally scalable, but introduces a broker dependency that isn't justified until check volume demands it.
