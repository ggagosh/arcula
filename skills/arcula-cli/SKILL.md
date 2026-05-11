---
name: arcula-cli
description: Use this skill whenever the user wants to use Arcula, sync MongoDB databases between environments, manage Arcula connections, import .env MongoDB URIs, create or approve sync plans, run/revert Arcula operations, or make Arcula safe for agent-driven database workflows. Prefer this skill for any task mentioning arcula, MongoDB environment sync, protected/prod database refreshes, backup-before-restore workflows, or agent-friendly database CLI usage.
---

# Arcula CLI

Arcula is a MongoDB sync CLI built around `mongodump`/`mongorestore`. It supports secure named connections, JSON output for agents, saved sync plans, OS-backed human approvals for protected targets, operation audit records, and backup-based reverts.

Use this skill to operate Arcula safely. The default posture is: inspect first, create a plan, require approval when policy says so, run only approved/safe plans, and keep enough metadata to revert.

## Core safety rules

- Prefer saved plan flow for any mutating sync: `sync plan` → optional `plan approve` by human → `operation run`.
- Use JSON for agent-readable commands: `--format json`; use `--agent` for agent execution commands.
- Arcula does not load `.env` by default. Use stored connections by default; add `--env` only when the user explicitly wants to read a project `.env` file or import it.
- Do not ask the user to reveal raw MongoDB URIs unless the task is explicitly connection setup. Never print raw URIs or passwords in your final answer.
- Do not bypass protected/prod policy with direct URIs, `--to-kind`, or edited metadata unless the user explicitly asks to reconfigure policy.
- For protected/prod targets, destructive actions (`--drop true` or `--clear true`) require `--backup true`, a successful backup, and human approval when policy requires it.
- Agents cannot provide real human approval. If Arcula says approval is required, tell the user to run the approval command in their own terminal, then wait for confirmation before continuing.
- Do not run raw `mongodump`/`mongorestore` directly for sync operations unless the user explicitly wants to bypass Arcula.

## Discovery checklist

Start with safe read-only discovery:

```bash
arcula --format json connection list
arcula --format json info
```

If a specific connection will be used, test it before planning. Use the command timeout mechanism available in your agent harness so unreachable hosts do not hang indefinitely.

```bash
arcula --format json connection test SOURCE
arcula --format json connection test TARGET
```

If the user upgraded from an older Arcula version with per-connection Keychain items, migrate those existing secure-storage entries into the single connection vault:

```bash
arcula connection migrate-vault
arcula --format json connection list
```

If the user wants to migrate from a project `.env`, load it explicitly:

```bash
arcula --env connection import-env --force
arcula --format json connection list
```

## Connection management

Use stored connection names in sync commands (`--from dev --to prod`) rather than raw URIs.

Add a connection interactively when a human can type the URI:

```bash
arcula connection add prod --kind prod --protected
```

For automation, prefer stdin over `--uri` to avoid shell history:

```bash
printf '%s' "$MONGO_URI" | arcula connection add prod --kind prod --protected --uri-stdin --force
```

Common kinds:

- `local` / `dev`: usually can allow agent apply and may not force backup.
- `staging`: decide by policy; often protected if it mirrors production.
- `prod`: protected by default, requires backup and human approval for destructive operations.

Inspect policy without revealing raw secrets:

```bash
arcula connection list
arcula connection show prod
```

## Preferred sync workflow

### 1. Create a plan

Create a non-mutating saved plan. Use `--backup true` for protected targets and any operation you may want to revert.

```bash
arcula sync plan \
  --from SOURCE \
  --to TARGET \
  --db SOURCE_DB \
  --target-db TARGET_DB \
  --backup true \
  --drop true \
  --format json
```

If source and target database names are the same, omit `--target-db`.

Capture the returned `data.id` as `PLAN_ID`. Check the returned flags:

- `requires_human_approval`
- `requires_full_backup`
- `target_protected`
- `warnings`

Show the human-readable plan if the user needs to review it:

```bash
arcula plan show PLAN_ID
```

### 2. Approval gate

If the plan requires human approval, do not attempt to approve it as an agent. Tell the user:

```bash
arcula plan approve PLAN_ID
```

Explain that this should trigger local OS user presence (for example macOS sudo/Touch ID/password or Linux sudo/polkit depending on setup). Continue only after the user says it is approved.

### 3. Run the operation

Run the saved plan in agent mode:

```bash
arcula operation run PLAN_ID --agent --format json
```

Capture the returned operation id as `OPERATION_ID`. Check `status`, `sync_report.backup_path`, and `error`.

### 4. Inspect operation records

```bash
arcula --format json operation list
arcula --format json operation show OPERATION_ID
```

If the operation failed before import because backup/export/connectivity failed, report that no destructive import should have occurred. If the operation failed during import, check whether Arcula restored the backup and report `restored_from_backup` from the sync report when present.

## Revert workflow

Only completed operations with a backup path can be reverted.

Preview first:

```bash
arcula operation revert OPERATION_ID --dry-run --format json
```

If the user confirms, revert for real:

```bash
arcula operation revert OPERATION_ID --confirm OPERATION_ID --format json
```

Treat revert as a mutating operation. If the target policy requires human approval, Arcula may ask for OS user presence.

## Dev-only fast path

For dev/local targets where policy allows agent apply, the saved plan flow still works and is preferred. If the user explicitly asks for a quick dev refresh, immediate run is acceptable only after dry-run and only for non-protected targets:

```bash
arcula sync run --agent \
  --from SOURCE \
  --to DEV_TARGET \
  --db SOURCE_DB \
  --backup false \
  --drop true \
  --dry-run
```

Then, if safe and requested:

```bash
arcula sync run --agent \
  --from SOURCE \
  --to DEV_TARGET \
  --db SOURCE_DB \
  --backup false \
  --drop true
```

Do not use this fast path for prod/protected targets.

## Legacy `.env` usage

Use `--env` only when the user explicitly wants Arcula to load `.env` from the current directory:

```bash
arcula --env info
arcula --env connection import-env --force
arcula --env sync plan --from LOCAL --to DEV --db my_database --backup true
```

After importing, prefer stored connections without `--env`.

## Troubleshooting patterns

- `requires human approval`: show `arcula plan approve PLAN_ID` and wait for the user.
- `without a full backup`: recreate the plan with `--backup true` or change policy only if the user explicitly requests it.
- `failed to connect` or timeout during backup/export/import: test the relevant connection and report connectivity, VPN, firewall, or MongoDB auth as likely causes.
- Repeated macOS Keychain prompts after upgrading: run `arcula connection migrate-vault` once. It migrates existing old per-connection Keychain items into the single connection vault.
- `No matching entry found in secure storage`: first try `arcula connection migrate-vault`; if the URI only exists in `.env`, run `arcula --env connection import-env --force`; otherwise ask the user to re-add the connection.
- `operation has no sync report`: it failed before a successful sync and cannot be reverted.

## Response style

When reporting to the user, include:

- plan id and operation id when created
- source and target connection/database names
- whether target is protected/prod
- whether backup was required and created
- whether human approval is required or already present
- exact next command for the human, if approval is needed

Do not include raw MongoDB connection strings, passwords, or full stderr containing credentials.
