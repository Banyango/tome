.PHONY: help build test fmt docs docs-build

help:
	@echo "make build       build tome (debug)"
	@echo "make test        run the tests"
	@echo "make fmt         format the code"
	@echo "make docs        serve the docs at http://localhost:3000, rebuilding on change"
	@echo "make docs-build  build the docs into docs/book"

build:
	cargo build

test:
	cargo test

fmt:
	cargo fmt

# Needs mdbook 0.5 (brew install mdbook); the theme in docs/theme depends on it.
docs:
	mdbook serve docs --open

docs-build:
	mdbook build docs
