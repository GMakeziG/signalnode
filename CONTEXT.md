# SignalNode Context

## Purpose

SignalNode is a modern, infrastructure-focused monitoring platform.

It starts with website uptime, SSL certificate, and API endpoint monitoring, but is designed to grow beyond WordPress into DevOps, SaaS, server, and Kubernetes monitoring.

## Product Positioning

SignalNode is not just a WordPress plugin.

It is a monitoring platform with:
- a Rust monitoring engine
- a REST API/backend
- an optional lightweight node agent
- a future web dashboard
- a WordPress plugin integration
- future SaaS and Kubernetes capabilities

## Repository Structure

```text
SignalNode
├── signalnode-core        # Rust monitoring engine
├── signalnode-api         # REST API/backend
├── signalnode-agent       # Optional lightweight node agent
├── signalnode-web         # Future dashboard
├── signalnode-wp-plugin   # WordPress plugin
├── docs
└── .github
