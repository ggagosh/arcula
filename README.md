# Arcula - MongoDB Database Synchronization Tool

[![CI](https://github.com/ggagosh/arcula/actions/workflows/ci.yml/badge.svg)](https://github.com/ggagosh/arcula/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

Arcula is a CLI application for synchronizing MongoDB databases between different environments. Named after the Roman god of transitions and passages, Arcula allows you to easily export databases from one MongoDB instance and import them to another.

## Features

- Export databases from one MongoDB instance
- Import databases to another MongoDB instance
- Dynamic environment configuration (not limited to predefined environments)
- Create and restore backups
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

Create a `.env` file in the root directory of the project with the following variables:

```
# MongoDB Connection URIs - You can add any environment you need
MONGO_LOCAL_URI=mongodb://localhost:27017
MONGO_DEV_URI=mongodb://user:password@dev.example.com:27017
MONGO_STG_URI=mongodb://user:password@stg.example.com:27017
MONGO_PROD_URI=mongodb://user:password@prod.example.com:27017
MONGO_GIO_URI=mongodb://user:password@gio.example.com:27017

# Path to MongoDB binaries (optional, auto-detected if not specified)
MONGODB_BIN_PATH=/usr/local/bin

# Backup directory
BACKUP_DIR=./backups

# Logging level: trace, debug, info, warn, error
RUST_LOG=info
```

You can copy the `sample.env` file and modify it for your needs. The application will dynamically detect all MongoDB environments from environment variables following the pattern `MONGO_<ENV>_URI`.

## Usage

### Display information about available environments

```bash
cargo run -- info
```

This command will show all configured MongoDB environments and their databases.

### Synchronize databases between environments

Interactive mode (will prompt for missing options):

```bash
cargo run -- sync
```

With command-line options:

```bash
cargo run -- sync --from LOCAL --to DEV --db my_database --backup true
```

Options:
- `--from`: Source environment (any configured environment)
- `--to`: Target environment (any configured environment)
- `--db`: Database to synchronize
- `--target-db`: Target database name (defaults to source database name)
- `--backup`: Whether to create a backup before import (true/false, defaults to true)
- `--drop`: Whether to drop collections during import (true/false, defaults to true)
- `--clear`: Whether to clear collections during import (true/false, defaults to false, ignored if drop is enabled)
- `--interactive`: Enable interactive prompts

### Examples

```bash
# Synchronize 'users' database from DEV to LOCAL environment with interactive prompts
cargo run -- sync --from DEV --to LOCAL --db users --interactive

# Synchronize 'products' database from PROD to STG environment without prompts
cargo run -- sync --from PROD --to STG --db products

# Synchronize 'analytics' database from GIO to DEV environment with custom target db
cargo run -- sync --from GIO --to DEV --db analytics --target-db analytics_copy
```

## Contributing

Contributions are welcome! Feel free to submit a pull request with your changes.

## License

MIT