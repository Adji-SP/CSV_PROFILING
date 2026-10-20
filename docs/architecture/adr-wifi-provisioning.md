# ADR: Board-local Wi-Fi provisioning

Status: Implemented; hardware acceptance remains required.

## Context

Build-time Wi-Fi credentials required another image for every network change.
Setup must work while the board cannot reach the dashboard.

## Decision

One task owns the modem independently of OTA HTTP requests. It reconnects and
opens a protected AP after 60 seconds offline, immediately if unprovisioned.
Two offline HTML pages and captive DNS provide the browser setup workflow.

Board NVS is authoritative. Optional build-time credentials support migration.
Only successful association plus DHCP commits a versioned record, alternating
between two keys to retain the prior valid record during interrupted writes.
SQLite stores device metadata, not Wi-Fi secrets.

## Alternatives and trade-offs

- Central-only storage cannot bootstrap an offline board and collects Wi-Fi secrets.
- Compile-time settings are simple but require rebuilding for network changes.
- BLE needs another client workflow; AP plus browser meets the phone/laptop need.
- The HTTP portal uses WPA2, a random per-board secret, AP-only socket checks,
  canonical Host/Origin checks, JSON, a session nonce and no-store responses.
  NVS still needs a separate production-encryption decision. No irreversible
  security settings are changed here.
- AP+STA can change radio channels while connecting. The UI explains reconnection
  and does not treat a lost HTTP connection as proof of success.
- No application GPIO is reserved for a setup button in this revision.
- OTA and MQTT endpoints remain separate from Wi-Fi provisioning.

## Consequences

Managed builds include setup automatically. Uploaded application functions,
peripherals and CSV functionality stay unchanged. Ordinary OTA preserves settings
with compatible partitions/schema. Slow OTA requests no longer block Wi-Fi setup.

Revisit for production encryption, enterprise Wi-Fi, fleet provisioning, a
board-specific button, or editable service endpoints.
