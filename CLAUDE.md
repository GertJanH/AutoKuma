# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this repository is

This is `GertJanH`'s fork of [BigBoot/AutoKuma](https://github.com/BigBoot/AutoKuma) — a Rust tool that automates Uptime Kuma monitor creation from Docker container labels (and files/Kubernetes CRs). `origin` is this fork (`github.com/GertJanH/AutoKuma`), `upstream` is `BigBoot/AutoKuma`.

**Purpose of this fork:** carry a small number of patches not yet merged upstream, and build custom Docker images from them for the homelab (`Docker_Glowstone`/`Docker_ME-Drive`/`Docker_VPS` repos on the user's own Gitea, each running an `autokuma` container).

**Branch layout:** `master` tracks upstream untouched. Each patch lives on its own branch off `master` (upstreamable as a PR on its own). The `homelab` branch = `master` + every carried patch cherry-picked on top, and is what the production image is built from: `.github/workflows/homelab-image.yml` (exists only on `homelab`) turns every push to that branch (except docs-only: `paths-ignore` `**.md`/`docs/**`) into `ghcr.io/gertjanh/autokuma:homelab-<run number>` plus a GitHub release of the same name with generated notes, which Renovate picks up (with those release notes) in the 3 homelab repos (procedure in `~/scripts/kuma-tools/README.md`). Upstream's own workflows trigger on tags and `master` pushes, but aren't registered in the fork yet (GitHub registers a workflow only on its first triggering push, so `gh workflow disable` 404s until then). Right after the first `master` sync to `origin`, disable them with `gh workflow disable <file> -R GertJanH/AutoKuma` (`docker-build-push.yml`, `rust-build-release.yml`, `create-release.yml`, `docs.yml`, `playground-pages.yml`); that first run is harmless (the docker one fails, it can't push to `ghcr.io/bigboot`). Don't push tags to `origin` before that. This `CLAUDE.md` lives only on `homelab`, not `master`. Currently carried on `homelab`:
- upstream PR #196 (s-nilsson, `fix/sync-recover-from-server-error`, fixes #191): drop the cached client after a session-lost `ServerError`/`LoginError`.
- notification-loop fix (upstream PR #198, branch `fix/notification-applyexisting-loop`; raw-ID docs + test are PR #199, branch `docs/raw-id-references`; both opened 2026-10-02, without Claude attribution): `applyExisting` added to `IGNORE_ATTRIBUTES` in `kuma-client/src/models/notification.rs` — Kuma adds it to a stored notification config, and since `merge_entities` (`serde_merge::omerge`) is a *shallow* merge the desired config never has it, so every sync re-applied the notification. Also documents raw-ID `notification_id_list` (`{"<id>": true}`; already worked via the lenient map parser).

- `feat/kuma-tags` (fork-only, branched from `homelab` because its test extends the raw-ID test from `docs/raw-id-references`): new monitor property `kuma_tags` (`"Containers, Pihole:primary"`) — Kuma tag *names*, resolved to ids in `kuma.rs::resolve_kuma_tags()`, called from `sync.rs` right after the sources produce entities (before the diff, so the resolved tags count as desired state — resolving later would make every sync strip and re-add them). Missing tags are created grey (`DEFAULT_TAG_COLOR`), duplicate names → oldest id + warning. Parser: `kuma-client/src/models/tag.rs::DeserializeTagValuesLenient` (string *and* list/object form — the latter is required because `merge_entities` round-trips entities through JSON). Unlikely to go upstream (maintainer prefers AutoKuma-managed entities, see #160).

Raw Kuma IDs (checked 2026-10-02, no code change needed): notifications via `notification_id_list: {"<id>": true}`, tags via `tags: [{"tag_id": <id>, "value": ...}]` — both work in labels and `AUTOKUMA__DEFAULT_SETTINGS` (test `entity::tests::default_settings_raw_ids`, docs in `docs/autokuma/usage.md`). Status pages have no equivalent: monitors don't reference a status page; a status-page entity owns its whole group list, so "add this monitor to an existing, non-AutoKuma page" would be a new feature — deliberately not built, the homelab repos' `sync_status_page.py` CI step covers it.

No local `cargo`: run tests in the build image, e.g. `docker run --rm -v "$PWD":/src -w /src rust:1.89 cargo test`. One doctest (`kuma-client/src/client.rs` ~line 1399) fails on upstream too.

## Commands

Cargo workspace with 4 members (`autokuma`, `autokuma-playground`, `kuma-cli`, `kuma-client`):

```bash
cargo build --release          # builds all workspace members
cargo build -p autokuma        # build just the daemon
cargo build -p kuma-cli        # build just the `kuma` CLI
cargo test                     # run all tests (few currently exist — config.rs, util.rs, migrations/v3.rs, kuma-client/src/config.rs)
cargo test -p autokuma <name>  # run a single test by name, scoped to one crate
```

Docker images — two separate Dockerfiles, both plain multi-stage `cargo install --path ...` builds against `rust:1.89` onto a `gcr.io/distroless/cc-debian13:debug` base (no special build tooling):

```bash
docker build -t autokuma .                 # Dockerfile: both `autokuma` + `kuma` CLI binaries, the production image
docker build -f Dockerfile.cli -t kuma .    # Dockerfile.cli: `kuma` CLI only, published separately for CLI-only users
```

`ARG FEATURES` is passed through to `cargo install --features "$FEATURES"` for both — see the `[features]` table in `autokuma/Cargo.toml` (`kubernetes` is on by default, `tokio-console`, `uptime-kuma-v1` are opt-in).

## Architecture

**`kuma-client`** is the shared Socket.IO-based API client against Uptime Kuma — both `autokuma` and `kuma-cli` depend on it (`autokuma` additionally enables its `private-api` feature). Per-entity-type models live under `kuma-client/src/models/` (`monitor`, `notification`, `tag`, `docker_host`, `status_page`, `maintenance`) and mirror Kuma's own Socket.IO payload shapes closely — when something fails with a cryptic serde error (e.g. `data did not match any variant of untagged enum OneOrMany`), the mismatch is almost always here: a hand-built or partial payload not matching exactly what `kuma-client`'s structs (de)serialize.

**`autokuma`** (the daemon) is a reconciliation loop with three layers:
- **Sources** (`src/sources/`): `docker_source.rs` (container labels), `file_source.rs` (`.json`/`.toml` files), `kubernetes_source.rs` (feature-gated on `kubernetes`, CRs — see `autokuma/kubernetes/crds-autokuma.yml`). `sources/mod.rs::get_sources()` picks which are active based on `state.config.{docker,files,kubernetes}.enabled`. Each source's job is only to produce `Entity` values — it knows nothing about Kuma itself.
- **`entity.rs`**: `Entity` is the unified in-memory representation of *any* Kuma object (monitor/group/notification/tag/docker-host/status-page/...), regardless of which source produced it. Docker labels (`<prefix>.<id>.<type>.<setting>`, see `docs/autokuma/usage.md` before guessing at label grammar — it's less obvious than it looks, e.g. templating/snippets/static-monitor file loading all interact with this) get grouped and parsed into `Entity`s here.
- **`sync.rs`**: compares the desired `Entity` set against live Kuma state (via `kuma-client`) and creates/updates/deletes to converge. `is_connection_error()` (around line 221) classifies a `kuma_client::Error` to decide whether a cached, authenticated client gets dropped and reconnected on failure — upstream PR #196 (carried on `homelab`) extends it to also treat a session-lost `ServerError`/`LoginError` as a connection error, so a stuck client gets dropped.

**`app_state.rs`** holds shared state plus a `sled` (embedded KV store) database mapping each managed entity's identity to the numeric Kuma ID AutoKuma created for it, so re-syncs update rather than duplicate. Schema changes go through `migrations/` (`v1.rs`/`v2.rs`/`v3.rs`, applied in order against the sled db on startup).

**`config.rs`** loads config via the `config` crate with a nested `AUTOKUMA__SECTION__KEY` env-var convention (double underscore = nesting) — see `docs/autokuma/configuration.md` for the full reference rather than inferring it from field names.

**`server.rs`/`metrics.rs`**: the built-in HTTP server (added v2.1.0-rc.1/rc.2) serving `/health` and `/metrics` (Prometheus-compatible) on port 8090 — this is what a container's `HEALTHCHECK`/Docker healthcheck should target instead of a custom staleness probe.

**`kuma-cli`** is a separate binary (`kuma`), one subcommand group per Kuma resource type (`monitor`/`tag`/`notification`/`maintenance`/`status-page`/`docker-host`), each in its own file under `kuma-cli/src/` (`monitor.rs`, `tag.rs`, etc.) — thin wrappers over `kuma-client` plus CLI argument parsing (`cli.rs`) and a local `database.rs` for its own state. Independent from `autokuma`'s reconciliation logic; only the underlying `kuma-client` is shared.

**`autokuma-playground`** is a separate web-based interactive demo (deployed via `.github/workflows/playground-pages.yml`) — not relevant to the fork's patch work above.

**`docs/`** is the source for the published documentation site (`zensical.toml`, built by `.github/workflows/docs.yml`) — treat `docs/autokuma/usage.md` and `docs/entity-types/*.md` as the authoritative label/config reference, not something to reverse-engineer from source alone. `ENTITY_TYPES.md` at the repo root is a generated/reference summary of the same entity-type docs.
