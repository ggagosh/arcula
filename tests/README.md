# MongoDB Importer Tests

This directory contains integration tests for the MongoDB Importer tool. The tests focus on realistic testing of the tool's functionality with actual MongoDB instances.

## Test Files

- **docker_tests.rs**: Integration tests that use real MongoDB instances
  - Tests the connection to MongoDB
  - Tests database export/import functionality
  - Tests backup and restore
  - Tests the full synchronization workflow

## Running Tests

Run all tests with:

```bash
cargo test
```

Run only the MongoDB integration tests:

```bash
cargo test --test docker_tests
```

## Test Configuration Options

The Docker tests are designed to be flexible and can operate in various environments:

### Using Docker Containers (Default)

By default, the tests will:
1. Start MongoDB containers on ports 27118 and 27119
2. Run tests against these containers
3. Clean up the containers when done

### Using External MongoDB Instances

You can provide MongoDB connection strings via environment variables to test against existing databases:

```bash
# Use existing MongoDB instances
export TEST_MONGO_SOURCE_URI=mongodb://localhost:27017
export TEST_MONGO_TARGET_URI=mongodb://localhost:27018
cargo test --test docker_tests
```

This is particularly useful in CI environments where MongoDB might be provided as a service.

### Test Behavior

The tests will automatically:

1. Check if MongoDB URIs are provided via environment variables
2. If URIs are available, use those for testing
3. If URIs are not available, create Docker containers for testing
4. Run all tests against the provided/created MongoDB instances
5. Clean up any resources when done

## CI Integration

For CI environments, use the external MongoDB option:

```yaml
# Example GitHub Actions configuration
jobs:
  test:
    runs-on: ubuntu-latest
    services:
      mongodb-source:
        image: mongo
        ports:
          - 27017:27017
      mongodb-target:
        image: mongo
        ports:
          - 27018:27017
    steps:
      - uses: actions/checkout@v2
      - name: Run tests
        run: |
          export TEST_MONGO_SOURCE_URI=mongodb://localhost:27017
          export TEST_MONGO_TARGET_URI=mongodb://localhost:27018
          cargo test
```

## Test Structure

Each test follows the same pattern:
1. Check if tests should be skipped
2. Determine if Docker containers are needed
3. Set up test environment (Docker or external)
4. Execute test operations against MongoDB
5. Verify results
6. Clean up resources

This flexible approach ensures tests can run in various environments while providing realistic testing of the application's functionality.