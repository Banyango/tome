.PHONY: help build release test fmt docs docs-build

help:
	@echo "make build       build tome (debug)"
	@echo "make release     build a self-contained release binary (compiles DuckDB, takes a few minutes)"
	@echo "make test        run the tests"
	@echo "make fmt         format the code"
	@echo "make docs        serve the docs at http://localhost:3000, rebuilding on change"
	@echo "make docs-build  build the docs into docs/book"

build:
	cargo build

# DuckDB is compiled in, so the binary needs no libduckdb next to it.
release:
	cargo build --release --features bundled

test:
	cargo test

fmt:
	cargo fmt

# Needs mdbook 0.5 (brew install mdbook); the theme in docs/theme depends on it.
docs:
	mdbook serve docs --open

docs-build:
	mdbook build docs
