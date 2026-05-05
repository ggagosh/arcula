# Arcula - MongoDB Database Synchronization Tool

[![CI](https://github.com/ggagosh/arcula/actions/workflows/ci.yml/badge.svg)](https://github.com/ggagosh/arcula/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/arcula.svg)](https://crates.io/crates/arcula)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

> Arcula started as a small workflow tool for moving MongoDB databases between local, development, staging, and other controlled environments. It has grown into a safer sync CLI built around `mongodump`/`mongorestore`, with secure connection storage, explicit sync plans, backups, approvals, operation records, and backup-based revert support. It is designed to make routine database refreshes convenient while reducing the risk of destructive mistakes.

Arcula is a CLI application for synchronizing MongoDB databases between different environments. It allows you to easily export databases from one MongoDB instance and import them to another.

## Features

- Export databases from one MongoDB instance
- Import databases to another MongoDB instance
- Secure connection manager backed by the OS keychain/keyring
- Dynamic environment configuration for legacy, CI, and one-off use
- Saved sync plans with hash-bound approvals
- Operation audit records and backup-based revert support
- OS-backed human approval gate for protected/production targets
- Create, verify, and restore backups
- Interactive mode with prompts for missing options
- Progress indicators for long-running operations
- Colored terminal output
- Automatic detection of MongoDB tools

## Installation

### Prerequisites

- MongoDB Tools (`mongodump` and `mongorestore` executables)
- Rust and Cargo (install from https://rustup.rs)

### Build from source

```bash
# Clone the repository
git clone https://github.com/ggagosh/arcula.git
cd arcula

# Build the project
cargo build --release

# The binary will be available at target/release/arcula
```

### Running with cargo

```bash
# Run directly with cargo
cargo run -- [COMMAND] [OPTIONS]
```

## Configuration

### Recommended: secure connection manager

Store connection names and safety metadata in Arcula, while the raw MongoDB URI is stored in your OS secure storage:

- macOS: Keychain
- Windows: Credential Manager
- Linux: kernel keyring

```bash
# Prompts securely for the URI, then stores it in the OS keychain/keyring
arcula connection add dev --kind dev
arcula connection add prod --kind prod

# Or pipe/pass a URI for automation. Prefer --uri-stdin over --uri to avoid shell history.
printf '%s' 'mongodb://user:password@dev.example.com:27017' \
  | arcula connection add dev --kind dev --uri-stdin --force
```

List and inspect configured connections without exposing raw credentials:

```bash
arcula connection list
arcula connection show prod
arcula connection test dev
```

Sync can then use the stored names:

```bash
arcula sync --from dev --to prod --db my_database --backup true
```

Stored connection metadata lives at your platform config path (or `ARCULA_CONFIG_DIR`) in `connections.json`; the URI itself is not written there. Plans, approvals, operations, and local audit metadata live under your platform data path (or `ARCULA_DATA_DIR`). The metadata format includes:

```json
{
  "connections": [
    {
      "name": "PROD",
      "kind": "prod",
      "protected": true,
      "secret_ref": "connection:PROD",
      "policy": {
        "allow_as_source": true,
        "allow_as_target": true,
        "allow_agent_apply": false,
        "human_approval_required": true,
        "destructive_requires_backup": true,
        "backup_verification_required": true
      }
    }
  ]
}
```

Production/protected targets cannot be dropped or cleared unless `--backup true` is enabled and the backup succeeds. They also require the saved plan + OS-backed approval flow before execution.

Adjust per-connection policy flags when needed:

```bash
arcula connection policy dev \
  --allow-agent-apply true \
  --human-approval-required false \
  --destructive-requires-backup false

arcula connection policy prod \
  --allow-agent-apply false \
  --human-approval-required true \
  --destructive-requires-backup true \
  --backup-verification-required true
```

Protected/prod connections always keep a safety floor: agent apply is disabled, human approval is required, destructive backup is required, and backup verification is required.

### Legacy / CI: `.env` and direct URIs

`.env` is still supported for CI, Docker, and one-off use:

```
# MongoDB Connection URIs - You can add any environment you need
MONGO_LOCAL_URI=mongodb://localhost:27017
MONGO_DEV_URI=mongodb://user:password@dev.example.com:27017
MONGO_STG_URI=mongodb://user:password@stg.example.com:27017
MONGO_PROD_URI=mongodb://user:password@prod.example.com:27017

# Optional environment metadata for safety and agent use
# Values: local, dev, staging, prod, other
MONGO_LOCAL_KIND=local
MONGO_DEV_KIND=dev
MONGO_STG_KIND=staging
MONGO_PROD_KIND=prod

# Optional extra protection for non-prod targets
# MONGO_STG_PROTECTED=true

# Path to MongoDB binaries (optional, auto-detected if not specified)
MONGODB_BIN_PATH=/usr/local/bin

# Backup directory
BACKUP_DIR=./backups

# Logging level: trace, debug, info, warn, error
RUST_LOG=info
```

Import existing `.env` entries into secure storage:

```bash
arcula connection import-env
```

Arcula dynamically detects all stored connections plus `.env` variables following `MONGO_<ENV>_URI`. It also reads optional metadata from `MONGO_<ENV>_KIND` (or `_TYPE` / `_ROLE`). If omitted, `LOCAL`, `DEV`, `STG`, and `PROD` are inferred from the name.

## Usage

### Display information about available environments

```bash
cargo run -- info
```

This command will show all configured MongoDB environments and their databases.

### Safer plan / approve / operation flow

For protected/production targets, and whenever you want an auditable workflow, use:

```bash
# 1. Create a saved sync plan. No database changes happen here.
arcula sync plan --from DEV --to PROD --db my_database --backup true

# 2. Review as text or Markdown.
arcula plan show <plan-id>
arcula plan show <plan-id> --markdown

# 3. Approve. Protected plans require an interactive terminal and OS user presence
#    via macOS/Linux sudo, with Linux polkit/pkexec attempted first when available.
arcula plan approve <plan-id>

# 4. Execute and save operation/audit metadata.
arcula operation run <plan-id>

# 5. Revert from the backup created before the operation, if needed.
arcula operation revert <operation-id> --dry-run
arcula operation revert <operation-id> --confirm <operation-id>
```

Agents can create plans and run safe/dev plans when the target connection policy allows `allow_agent_apply=true`:

```bash
arcula --no-env sync plan --agent --from PROD --to DEV --db my_database
arcula --no-env operation run <plan-id> --agent
```

For protected/prod targets, agent execution returns a human-approval-required error until a human runs `arcula plan approve <plan-id>` from an interactive terminal. Passwordless `sudo` is rejected as an approval provider because it does not prove user presence.

### Immediate synchronization

Interactive mode (will prompt for missing options):

```bash
cargo run -- sync
```

With command-line options:

```bash
cargo run -- sync --from LOCAL --to DEV --db my_database --backup true
```

Options:
- `--from`: Source stored connection or `.env` environment
- `--to`: Target stored connection or `.env` environment
- `--from-uri`: Source MongoDB URI; bypasses `.env` for source
- `--to-uri`: Target MongoDB URI; bypasses `.env` for target
- `--from-kind` / `--to-kind`: Environment kind override (`local`, `dev`, `staging`, `prod`, `other`)
- `--db`: Database to synchronize
- `--target-db`: Target database name (defaults to source database name)
- `--backup`: Whether to create a backup before import (true/false, defaults to true)
- `--drop`: Whether to drop collections during import (true/false, defaults to true)
- `--clear`: Whether to clear collections during import (true/false, defaults to false, ignored if drop is enabled)
- `--interactive`: Enable interactive prompts
- `--agent`: JSON output, no colors/progress, no prompts
- `--dry-run`: Output the sync plan without executing
- `sync plan`: Save a hash-bound plan for later approval/execution
- `plan approve`: Save an approval record; protected plans require OS user presence
- `operation run`: Execute a saved plan and record operation metadata
- `operation revert`: Restore the target DB from the operation's pre-sync backup
- `--format json`: Machine-readable output for `info` and sync plans/results
- `--no-env`: Do not load `.env`

### Examples

```bash
# Synchronize 'users' database from DEV to LOCAL environment with interactive prompts
cargo run -- sync --from DEV --to LOCAL --db users --interactive

# Synchronize 'products' database from PROD to STG environment without prompts
cargo run -- sync --from PROD --to STG --db products

# Synchronize 'analytics' database from RANDOM to DEV environment with custom target db
cargo run -- sync --from RANDOM --to DEV --db analytics --target-db analytics_copy

# Agent-friendly dry run using direct URIs, without reading .env
cargo run -- --no-env sync --agent \
  --from-uri mongodb://source:27017 \
  --to-uri mongodb://target:27017 \
  --to-kind prod \
  --db analytics \
  --backup true \
  --dry-run

# Agent-friendly saved plan flow for a policy-allowed dev target
cargo run -- --no-env sync plan --agent --from PROD --to DEV --db analytics
cargo run -- --no-env operation run <plan-id> --agent

# Protected/prod target flow with human approval
cargo run -- sync plan --from DEV --to PROD --db analytics --backup true
cargo run -- plan show <plan-id> --markdown
cargo run -- plan approve <plan-id>
cargo run -- operation run <plan-id>

# JSON environment/connection discovery
cargo run -- --format json info

# Manage secure stored connections
cargo run -- connection add dev --kind dev
cargo run -- connection add prod --kind prod
cargo run -- connection list
cargo run -- connection test dev
```

## Contributing

Contributions are welcome! Feel free to submit a pull request with your changes.

## License

MIT