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

## Language

**Monitor**:
A configuration object that defines what to check, how often, and under what conditions. Has a `kind` (uptime, ssl, api) that shapes its configuration. Carries both a failure threshold and a recovery threshold (consecutive CheckResults required to open or close an Incident). Lifecycle states: `active`, `paused`, `archived`.
_Avoid_: check, probe, watcher, sensor

**CheckResult**:
The recorded outcome of a single Monitor execution — status (`up`, `degraded`, `down`), latency, timestamp, and any error detail. Only `down` results count toward the failure threshold; `degraded` is informational.
_Avoid_: result, response, event, log entry

**Incident**:
A time-bounded period during which a Monitor is in an unhealthy state. Opens when a failure threshold is crossed (defined on the Monitor); closes when recovery is confirmed.
_Avoid_: outage, alert, event, downtime

**NotificationChannel**:
A configured destination for Incident notifications — e.g. email address, Slack webhook, or SMS number. Belongs to a Workspace.
_Avoid_: integration, contact, alert channel, destination

**Workspace**:
The organizational unit that owns Monitors, NotificationChannels, and Incidents. Users are members of a Workspace with a role (Owner or Member).
_Avoid_: account, organization, team, project, tenant

**User**:
A person who authenticates with SignalNode and belongs to one or more Workspaces. Has a role within each Workspace (Owner or Member).
_Avoid_: account, customer, admin (as a role name)

**StatusPage** _(reserved — Phase 2)_:
A public URL published by a Workspace that displays selected Monitor health and Incident history for external customers.
_Avoid_: status site, public dashboard

**MaintenanceWindow** _(reserved — Phase 2)_:
A scheduled period during which a Monitor's failures are suppressed and no Incidents are opened. Users can pause a Monitor manually in Phase 1.
_Avoid_: blackout, silence, downtime window

**Notification**:
A persisted record of a single dispatch attempt to a NotificationChannel, triggered by an Incident opening or closing. Tracks delivery status and timestamp.
_Avoid_: alert, message, event, ping

## Relationships

- A **Workspace** owns zero or more **Monitors** and **NotificationChannels**
- A **Monitor** produces many **CheckResults** over its lifetime
- A **Monitor** has zero or one open **Incident** at any time; past Incidents are closed
- A **Monitor** references one or more **NotificationChannels** directly
- An **Incident** produces one or more **Notifications** (on open and on close)
- A **Notification** targets exactly one **NotificationChannel**

## Example dialogue

> **Dev:** "When a **Monitor** goes down, do we immediately open an **Incident**?"
> **Domain expert:** "No — the Monitor's failure threshold has to be crossed first. If it's set to 2, we need 2 consecutive `down` **CheckResults** before an **Incident** opens."
>
> **Dev:** "And what if the check comes back `degraded` — slow but not down?"
> **Domain expert:** "`Degraded` is informational. It shows up in the **CheckResult** history but doesn't count toward the failure threshold and doesn't open an **Incident**."
>
> **Dev:** "Once an **Incident** is open, how does the **User** find out?"
> **Domain expert:** "The **Monitor** has one or more **NotificationChannels** configured. When the **Incident** opens, we dispatch a **Notification** to each of those channels and record the attempt."
>
> **Dev:** "Who can add a **NotificationChannel**?"
> **Domain expert:** "Any **User** who's a member of the **Workspace** — both Owners and Members can manage channels."

## Flagged ambiguities

- "Alert" was used in early planning to mean both the condition that fires and the message sent — resolved: **Incident** is the condition, **Notification** is the dispatched message. "Alert" is not a domain term.
- "Account" was used interchangeably with **Workspace** and **User** — resolved: these are distinct concepts; "account" is avoided entirely.

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
