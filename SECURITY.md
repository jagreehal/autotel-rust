# Security policy

## Reporting a vulnerability

Please report vulnerabilities privately. Do not open a public issue.

- Email **jag.reehal+security@gmail.com**, or
- use GitHub's private vulnerability reporting: [Security → Report a vulnerability](https://github.com/jagreehal/autotel-rust/security/advisories/new).

Include the crate version, a minimal reproducer, and the impact you expect. You should get a reply within 7 days.

## Trust model

autotel trusts W3C `baggage` from callers. An `autotel.debug` entry keeps that request's trace through tail sampling, and `.baggage(prefix)` copies entries onto spans. If callers outside your network can reach a service, strip `baggage` at your gateway or ingress.

## Supported versions

Fixes go into the latest published version. Older versions do not get backports.
