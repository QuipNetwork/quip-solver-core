# Developer entry points. CI calls the same targets, so a green local run and
# a green pipeline mean the same thing.

.PHONY: help
help:
	@echo "check-release        every publish path, dry-run only (no registry writes)"
	@echo "  check-crate-publish  cargo publish --dry-run for the whole workspace"
	@echo "  check-python-dist    build the wheel and sdist, then twine check"
	@echo "  check-npm-dist       build the wasm and TypeScript, then npm publish --dry-run"
	@echo "  check-clib           build the C library and pack the release tarball"
	@echo "check-examples       conformance gate for all four sample solvers"
	@echo "test                 fmt, clippy and the Rust test suite"
	@echo "lint                 shell and Python linters"

# -- Release preflight ------------------------------------------------------
#
# check-release is the gate the release pipeline runs BEFORE it publishes
# anything, and it runs on every pipeline rather than only on a tag. A broken
# release setup then fails on a merge request instead of halfway through a
# release, with some registries written and others not.
#
# Nothing here writes to a registry. Every target is a dry run or a local
# build.

.PHONY: check-release
check-release: check-crate-publish check-python-dist check-npm-dist check-clib

# `cargo package`, not `cargo publish --dry-run`. Both verify the same way, and
# both leave the .crate tarballs the smoke tests unpack and build against.
# `package` is preferred because it stays entirely local: it never enters the
# upload path, so this gate cannot fail on registry state. The release job runs
# the real `cargo publish --workspace`.
#
# The stale tarballs are deleted first. Cargo leaves an existing .crate in place
# when it does not write one, so a leftover from an earlier version silently
# becomes the thing the smoke tests validate.
#
# --locked so packaging resolves the committed Cargo.lock. Without it a drifted
# lock file passes here and fails in the real publish.
#
# The three excluded crates carry publish = false. `cargo publish` skips those
# on its own, but `cargo package` does not, and it fails on the first one whose
# path dependency has no version. Naming them is the price of keeping the
# tarballs.
.PHONY: check-crate-publish
check-crate-publish:
	find target/package -maxdepth 1 -name '*.crate' -delete 2>/dev/null || true
	cargo package --locked --workspace \
		--exclude quip-protocol-py \
		--exclude quip-protocol-wasm \
		--exclude quip-solver-c
	@ls -la target/package/*.crate

# Needs maturin and twine on PATH. twine check catches the metadata problems
# PyPI rejects on upload, which is the class of failure that otherwise only
# appears after crates.io has already been written.
#
# dist/python/ rather than dist/, for two reasons that both cost a real upload
# if ignored. A bare dist/ accumulates wheels across renames and version bumps,
# and `twine upload dist/*` would push every stale one. It also holds the C
# library staging tree, which twine rejects as an unknown distribution format.
.PHONY: check-python-dist
check-python-dist:
	rm -rf dist/python && mkdir -p dist/python
	maturin build --release --out dist/python
	maturin sdist --out dist/python
	twine check dist/python/*
	# twine check validates that the metadata is well formed, not that the
	# files it names exist. PyPI checks that at upload time and rejects the
	# sdist with 400, after the wheel of the same version has been accepted.
	bash scripts/check-sdist-license-files.sh dist/python/*.tar.gz
	bash scripts/check-sdist-stub-package.sh dist/python/*.tar.gz

# Two steps, for the same reason check-crate-publish uses `cargo package`.
#
# `npm publish --dry-run` validates the publish path but writes no tarball.
# `npm pack` writes the tarball the smoke test installs. Neither proves
# authentication: OIDC needs a real CI provider, so only the publish job
# exercises that.
.PHONY: check-npm-dist
check-npm-dist:
	cd npm && rm -f ./*.tgz && npm ci && npm publish --dry-run --tag latest && npm pack
	@ls -la npm/*.tgz

# The C library ships as a release asset, so packing it is part of the release
# path and belongs in the same gate as the three registries.
.PHONY: check-clib
check-clib:
	cargo build -p quip-solver-c --release
	rm -f dist/quip-solver-clib-*
	rm -rf dist/clib && mkdir -p dist/clib/include
	cp target/release/libquip_solver_c.so dist/clib/
	cp target/release/libquip_solver_c.a dist/clib/
	cp target/release/quip_solver.h dist/clib/include/
	cp LICENSE NOTICE dist/clib/
	cd dist/clib && tar czf ../quip-solver-clib-$(TAG)-linux-amd64.tar.gz .
	cd dist && sha256sum quip-solver-clib-$(TAG)-linux-amd64.tar.gz > quip-solver-clib-$(TAG)-linux-amd64.tar.gz.sha256

# Overridden by CI with the real tag. A local run wants a name, not a failure.
TAG ?= dev

# -- Conformance ------------------------------------------------------------

# SPEC section 6 defines conformance as a green quip-solver-drive run, so this
# is the same check a solver author runs against their own binary.
.PHONY: check-examples
check-examples:
	bash scripts/check-examples.sh

# -- Test -------------------------------------------------------------------

# The CI rust job runs this target rather than repeating the commands, so a
# green local run and a green pipeline cover the same crates.
#
# quip-protocol-py is the only crate left out of the workspace run. It is a PyO3
# cdylib, so linking it needs a Python interpreter that this target does not
# assume. Clippy is a separate step below because it stops before linking and
# therefore needs no interpreter.
#
# quip-protocol-wasm is NOT excluded. It builds for the host like any other
# crate; only its published artifact is wasm32. Excluding it here while CI
# included it meant a clippy failure in that crate reached the pipeline.
.PHONY: test
test:
	cargo fmt --all -- --check
	cargo clippy --workspace --exclude quip-protocol-py --all-targets -- -D warnings
	cargo clippy -p quip-protocol-py --all-targets -- -D warnings
	cargo test --workspace --exclude quip-protocol-py

# -- Lint -------------------------------------------------------------------

# The non-Rust linters. `test` covers Rust through cargo fmt and clippy.
#
# TypeScript is deliberately absent. `npm run build` runs tsc under
# `strict: true`, and the wasm job already runs it as a release gate, so a
# second type check here would either repeat that job's wasm-pack build or
# duplicate its result.
#
# shfmt is called with no style flags so it reads .editorconfig.
.PHONY: lint
lint:
	shellcheck scripts/*.sh
	shfmt -d scripts/
	ruff check .
	ruff format --check .

.PHONY: clean
clean:
	rm -rf dist
	cargo clean
