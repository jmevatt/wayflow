# Security Policy

## Supported versions

Wayflow is pre-1.0 and under active development. Only the latest commit on `master`
receives security fixes.

## Reporting a vulnerability

**Do not open a public GitHub issue for security vulnerabilities.**

Email: jordan@evattlabs.com

Include:
- A description of the vulnerability and its potential impact
- Steps to reproduce or a proof-of-concept if you have one
- Your name/handle if you want credit (optional)

You can expect an acknowledgement within a few days. There is no bug bounty program.

## Scope

Things that are in scope:

- Remote code execution via malformed network input
- Authentication or TLS bypass
- Privilege escalation via the input injection backends
- Clipboard data leakage to unintended processes

Things that are out of scope:

- Denial-of-service against a local attacker who already has network access
- Issues requiring physical access to the machine
- Theoretical vulnerabilities with no demonstrated impact

## Notes on the threat model

Wayflow operates on a trusted LAN. The TLS connection uses a self-signed server
certificate accepted on first connection (TOFU). There is no cryptographic
authentication of clients -- any machine that knows the server address can connect.
This is a known limitation and by design for the current phase.

Security-focused PRs are welcome -- improving the authentication model, hardening
the TLS configuration, or tightening input validation. Open an issue first to
discuss the approach, then send the PR (see CONTRIBUTING.md).
