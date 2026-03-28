# Open Source Prime Time Checklist

This document is the working board for making `switchboard` feel like a serious public Rust CLI repo instead of a strong internal prototype with a README addiction.

It is intentionally biased toward:

- prebuilt binaries over source-build friction
- generated docs over handwritten drift
- LLM operator ergonomics and predictable JSON
- honest support status over wishcasting

## Goals

- Make installation boring and fast on macOS, Linux, and Windows.
- Make the README explain the problem, the solution, how it works, and how to get started in one pass.
- Generate reference docs from live code and committed catalogs so they do not rot.
- Make the repo legible to both humans and LLMs.
- Publish clear support boundaries for stable, preview, planning-only, and raw surfaces.

## Problems Switchboard Solves

These need to show up in the README, docs, and website without sounding like marketing poison.

### Trust Boundary Problems

- Agent tooling is usually decent at reasoning and sloppy at authority boundaries.
- People need separate namespaces for work and personal identities on the same machine.
- Writes need to be explicit, reviewable, auditable, and ideally reversible when that can be done honestly.
- The durable contract should survive backend churn, CLI churn, and model churn.

### Multi-Login CLI Problems

- Many useful CLIs do not support multi-login cleanly on one machine.
- Google Workspace CLI is the canonical example: upstream login and cache behavior are not designed around clean local account isolation.
- `switchboard` solves that by treating each namespace as a separate authority domain, materializing credentials separately, and wiring per-namespace state directories so accounts do not stomp each other.
- This is not just "nice config", it is one of the actual product problems being solved.

### LLM Operator Problems

- LLMs need exact command shapes, explicit namespaces, stable JSON, and copy-pasteable examples.
- Handwritten docs drift and become lies faster than anyone admits.
- Raw provider CLIs are broad but ugly, curated tools are nicer but narrower, and both need to be discoverable without source-diving.

## Release Bar

Do not call the repo open-source-prime-time ready until all of these are true:

- Main install paths use prebuilt binaries.
- Release artifacts exist for supported targets and include checksums.
- README has install plus 5-minute quickstart plus real examples.
- Docs are generated from live catalogs and published automatically.
- Stable versus preview versus planning-only versus raw is visible everywhere.
- Public repo scaffolding exists: contributing, security, changelog, issue templates, PR template.

## Status Taxonomy

Use one taxonomy everywhere: README, docs site, generated reference, and release notes.

- `stable`: executable, documented, tested, and intended for normal use
- `preview`: executable but still subject to shape changes
- `planning_only`: plans cleanly but does not apply
- `raw`: inventory-derived provider passthrough, broad coverage but lower ergonomics

## Workstreams

### 1. Distribution And Release Engineering

- Add a release workflow for GitHub Releases.
- Use `cargo-dist` as the release backbone.
- Define the supported target matrix as `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`, `x86_64-apple-darwin`, `aarch64-apple-darwin`, and `x86_64-pc-windows-msvc`.
- Verify musl builds before promising static binaries on Linux.
- Publish checksums for every release.
- Publish `switchboard-cli` to crates.io.
- Ensure `cargo install --locked switchboard-cli` works.
- Ensure `cargo binstall switchboard-cli` works once release naming and metadata are in place.
- Add a Homebrew install path.
- Keep Nix as a supported install path.
- Add upgrade docs.
- Add uninstall docs.

### Acceptance Criteria

- A user can install `switchboard` without building Rust from source.
- The latest release page clearly shows supported targets and checksums.
- Install docs cover direct download, shell installer, PowerShell installer, Homebrew, Cargo, and Nix.

### 2. README Rewrite

- Rewrite `README.md` into a landing page, not a full design dump.
- Put this order near the top: problem, what switchboard is, why CLI first, install, 5-minute quickstart, real workflows, safety model, docs links, and support status.
- Move deep conceptual material into `docs/concepts/` and `docs/architecture/`.
- Add a short "who this is for" section covering humans, scripts, and LLM operators.
- Add one dead-simple quickstart with a real config snippet and real commands.
- Add one GitHub workflow example and one Google workflow example.

### Acceptance Criteria

- A new user can understand the product and run a first successful command from the README alone.
- The README explains the problem and solution before architecture trivia starts rambling.

### 3. Generated Reference Docs

- Build a docs generator on top of the existing inventory plus manifest pipeline.
- Generate docs from provider inventories, curated manifests, and the tool catalog, not from separate handwritten tables.
- Emit at least `docs/reference/catalog.json`, `docs/reference/tools/<tool>.md`, `docs/reference/providers/github.md`, and `docs/reference/providers/google.md`.
- Include for each tool: tool name, provider, kind, surface, execution support, undo support, aggregate read support, argument table, available namespaces, generated examples, generated notes, and support status.
- Reuse tested fixtures for example outputs where possible.
- Add a CI drift check so generated docs cannot silently go stale.

### Acceptance Criteria

- Reference docs can be regenerated in one command.
- CI fails if generated reference docs drift from code.
- Users can discover the tool surface without reading Rust.

### 4. LLM-First Docs

- Add `llms.txt` at the docs root.
- Add `docs/llms/getting-started.md`.
- Add `docs/llms/patterns.md`.
- Add a short rules page for model callers that says: prefer `--json`, pass `--ns` explicitly for writes, draft or plan writes first, repeat `--ns` only for aggregate reads, and treat raw tools as escape hatches rather than defaults.
- Publish copy-pasteable end-to-end workflows for Gmail search -> read -> draft, calendar create -> approve -> apply, and PR read -> comment draft -> approve -> apply.
- Make sure the docs remain accessible as raw Markdown, not only rendered HTML.

### Acceptance Criteria

- An LLM can discover safe calling patterns without reading source.
- Examples are short, exact, and stable enough to paste into prompts.

### 5. Public Repo Hygiene

- Add `CONTRIBUTING.md`.
- Add `SECURITY.md`.
- Add `CODE_OF_CONDUCT.md`.
- Add `CHANGELOG.md`.
- Add `.github/ISSUE_TEMPLATE/`.
- Add `.github/pull_request_template.md`.
- Fill package metadata for published crates: `description`, `repository`, `homepage`, `documentation`, `readme`, `keywords`, and `categories`.
- Decide which sibling crates are public now versus later.
- If a crate is not intended for public release yet, mark it clearly or keep it out of publishing.
- Generate shell completions.
- Generate man pages.

### Acceptance Criteria

- The repo has the expected public trust signals.
- Crates.io pages and release pages look deliberate instead of half-dressed.

### 6. Pages And Docs Publishing

- Turn GitHub Pages into a real docs and install surface.
- Keep the OAuth helper pages, but stop making them the whole public site.
- Publish generated docs to Pages automatically.
- Make the install page platform-aware.
- Link the latest release artifacts directly from the docs site.
- Add a docs navigation structure with install, quickstart, concepts, architecture, reference, and LLM guides.

### Acceptance Criteria

- A user can land on the Pages site and get the right install command in seconds.
- The docs site and the repo README tell the same story.

## Proposed PR Order

1. `release: add cargo-dist and GitHub release pipeline`
2. `docs: rewrite README into problem/install/quickstart structure`
3. `docs: generate reference docs from manifests and inventories`
4. `docs: add llms.txt and LLM operator guides`
5. `meta: add contributing security changelog and GitHub templates`
6. `site: publish generated docs on GitHub Pages`
7. `packaging: publish crates and add Homebrew install path`

## Notes For Messaging

When explaining the problem publicly, keep these points visible:

- `switchboard` is not trying to be another chat shell.
- The product is the stable local CLI contract plus trust boundary.
- One major pain point solved is making single-login-hostile CLIs workable for multiple accounts through namespace-scoped state and credential materialization.
- This matters a lot for Google Workspace style tooling.
- The repo is optimized for humans and LLMs, not just one of them.

## Launch Definition Of Done

- Install is boring.
- Docs are generated.
- README is sharp.
- LLM guidance is explicit.
- Releases are real.
- Support boundaries are honest.
- The repo looks like it respects the reader's time.
