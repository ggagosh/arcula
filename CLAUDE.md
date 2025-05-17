# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Essential Commands

### Building the Project

```bash
cargo build                # Build in debug mode
cargo build --release      # Build in release mode
```

### Running the Application

```bash
# Display information about available environments
cargo run -- info

# Synchronize databases in interactive mode
cargo run -- sync

# Synchronize databases with command-line options
cargo run -- sync --from LOCAL --to DEV --db my_database --backup true --drop true
```

### Testing

```bash
cargo test                 # Run all tests
cargo test <test_name>     # Run a specific test
```

### Linting and Code Quality

```bash
cargo clippy               # Run clippy for linting
cargo fmt                  # Format code with rustfmt
```

## Project Architecture

This is a MongoDB database synchronization CLI tool built with Rust. The project is organized as follows:

1. **Main CLI Interface** (`src/main.rs`): 
   - Defines the command-line interface using `clap`
   - Handles environment setup and command routing

2. **Commands** (`src/commands/`):
   - `info.rs`: Displays information about configured MongoDB environments
   - `sync.rs`: Handles database synchronization with interactive or non-interactive modes

3. **Core Logic** (`src/core/`):
   - `sync.rs`: Contains the core synchronization logic and configuration structures

4. **Utilities** (`src/utils/`):
   - `mongodb.rs`: MongoDB-specific utilities for database operations (export, import, backup, restore)

5. **Configuration** (`src/config/`):
   - Handles environment-specific configuration (LOCAL, DEV, STG, PROD)
   - Manages MongoDB connection strings and paths

## Key Workflows

1. **Database Synchronization**:
   - User selects source and target environments
   - User selects source and target databases
   - Optional backup of target database is created
   - Source database is exported using `mongodump`
   - Target database is imported using `mongorestore` with user-specified options (drop, clear)
   - Progress indicators show operation status

2. **Environment Information**:
   - Displays available environments from configuration
   - Shows connection status and available databases for each environment

## Development Notes

1. **Code Structure**:
   - The application uses a `SyncParams` struct in `commands/sync.rs` to organize command parameters
   - Use `execute_with_params(params: SyncParams)` rather than the deprecated `execute()` function with multiple parameters

2. **Dependencies**:
   - The application requires the MongoDB tools (`mongodump` and `mongorestore`) to be installed
   - The tools path is configurable via the `MONGODB_BIN_PATH` environment variable
   - If not configured, the application uses the `which` crate to find the tools in PATH
   - The application performs strict validation of MongoDB tools at startup
   - If MongoDB tools are not found, the application will exit with an appropriate error message

2. **Configuration**:
   - Environment configuration uses a `.env` file (copy from `sample.env`)
   - MongoDB environments are configured using environment variables in the format `MONGO_<ENV>_URI`
   - Any environment can be configured (not limited to LOCAL, DEV, STG, PROD)
   - Custom environments like `MONGO_GIO_URI` will be automatically detected
   - Backup directory is configurable via `BACKUP_DIR`
   - Logging level is controlled via `RUST_LOG`

3. **Error Handling**:
   - The application uses `anyhow` for error context and propagation
   - Detailed error messages are provided for MongoDB operations
   - Failed imports can trigger automatic backup restoration