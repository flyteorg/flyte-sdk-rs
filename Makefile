# Target configuration
.PHONY: all help dev clean build check test fmt lint

# Default target when just running `make`
all: fmt lint check test build

help:
	@echo "Available commands:"
	@echo "  make              - Run all the commands cargo {build check test fmt lint}"
	@echo "  make dev          - Run the application locally in development mode"
	@echo "  make build        - Compile the release binary"
	@echo "  make check        - Fast-check the code for compilation errors"
	@echo "  make test         - Run all unit and integration tests"
	@echo "  make fmt          - Automatically format code using rustfmt"
	@echo "  make lint         - Run clippy lints and check formatting"
	@echo "  make clean        - Remove the target build directory"

# Local Development
dev:
	cargo run

# Production Build
build:
	cargo build --release

# Fast Verification
check:
	cargo check

# Test Suite
test:
	cargo test --workspace --all-targets

# Code Formatting
fmt:
	cargo fmt --all

# Code Linting (Fails on warnings to maintain quality)
lint:
	cargo fmt --all -- --check
	cargo clippy --all-targets -- -D warnings

# Cleanup
clean:
	cargo clean