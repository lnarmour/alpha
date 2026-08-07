# macOS Homebrew installs pkg-config here; it's often missing from PATH in non-interactive
# shells, which breaks isl-sys's build.rs (it shells out to pkg-config to find libisl).
export PATH := /opt/homebrew/bin:$(PATH)

CARGO ?= uv run cargo

.PHONY: all build release test check clippy fmt lint wheel clean

all: build

build:
	$(CARGO) build --workspace

release:
	$(CARGO) build --workspace --release

test:
	$(CARGO) test --workspace

# Builds the alphalang wheel via maturin (alphalang's own uv-managed dev dependency, hence `cd`
# rather than running from the workspace root venv). ISL_STATIC=1 links libisl/libgmp statically
# into the extension module, so the wheel has no runtime dependency on them being installed —
# same convention release-alphalang.yml's CI build uses. Output: alphalang/dist/*.whl.
wheel:
	cd alphalang && ISL_STATIC=1 uv run maturin build --release -o dist

check:
	$(CARGO) check --workspace

clippy:
	$(CARGO) clippy --workspace --all-targets

fmt:
	$(CARGO) fmt --all

lint:
	uv run prek run --all-files

clean:
	$(CARGO) clean
