# Security Policy

## Supported versions

MeterMaid is pre-1.0; only the latest release on the default branch is supported with security fixes.

## Reporting a vulnerability

Please report security issues **privately** rather than opening a public issue.

- Preferred: open a [GitHub private security advisory](https://github.com/reverentgeek/metermaid/security/advisories/new).
- Or email **<david@reverentgeek.com>** with details and, if possible, steps to reproduce.

You can expect an acknowledgement within a few days. Once a fix is available it will be released and the reporter credited (unless you prefer to remain anonymous).

## Scope notes

MeterMaid is a local desktop app that captures audio from input devices. It performs no network requests and stores no credentials. The most relevant surface is the OS audio permission and the Tauri webview, which runs under a restrictive Content-Security-Policy (see `src-tauri/tauri.conf.json`).
