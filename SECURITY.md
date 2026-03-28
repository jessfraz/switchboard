# Security Policy

`switchboard` handles credentials, namespace isolation, approval gates, and write execution. Security bugs matter here more than they do in some toy CLI that just prints a cat fact.

## Please report privately

Do not open a public issue for:

- credential leakage
- auth bypasses
- namespace isolation failures
- write-safety or approval bypasses
- arbitrary command execution bugs

Use GitHub private vulnerability reporting for this repository if it is available. If that flow is unavailable in your environment, open a minimal public issue without exploit details and ask for a private contact channel.

## What to include

- affected command or tool name
- provider and namespace assumptions
- exact reproduction steps
- impact, especially whether reads, writes, or secrets are exposed
- logs, screenshots, or redacted config snippets if they help

## Response expectations

- We will acknowledge serious reports as quickly as practical.
- Fixes may land privately first if the bug affects credential handling or write safety.
- Please give us reasonable time to ship a fix before public disclosure.
