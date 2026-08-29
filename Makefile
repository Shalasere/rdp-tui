.PHONY: setup test test-python test-rust lint lint-python lint-rust format-check format-check-python format-check-rust run

setup:
	python -m venv .venv
	.venv/bin/python -m pip install --upgrade pip
	.venv/bin/python -m pip install -e '.[dev]'

test: test-python test-rust

test-python:
	.venv/bin/python -m unittest discover -s tests -v

test-rust:
	cargo test --locked --all-targets

lint: lint-python lint-rust

lint-python:
	.venv/bin/ruff check .

lint-rust:
	cargo clippy --locked --all-targets --all-features -- -D warnings

format-check: format-check-python format-check-rust

format-check-python:
	.venv/bin/ruff format --check .

format-check-rust:
	cargo fmt --check

run:
	.venv/bin/rdp-tui
