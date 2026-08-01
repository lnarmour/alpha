# macOS Homebrew installs pkg-config here; it's often missing from PATH in non-interactive
# shells, which breaks isl-sys's build.rs (it shells out to pkg-config to find libisl).
export PATH := /opt/homebrew/bin:$(PATH)

CARGO ?= cargo
ARGS ?=

.PHONY: all build release test check clippy fmt lint clean run

all: build

build:
	$(CARGO) build --workspace

release:
	$(CARGO) build --workspace --release

test:
	$(CARGO) test --workspace

check:
	$(CARGO) check --workspace

clippy:
	$(CARGO) clippy --workspace --all-targets

fmt:
	$(CARGO) fmt --all

lint:
	uvx prek run --all-files

clean:
	$(CARGO) clean

run: release
	./target/release/alphac $(ARGS)
