.PHONY: setup test lint format-check run

setup:
	python -m venv .venv
	.venv/bin/python -m pip install --upgrade pip
	.venv/bin/python -m pip install -e '.[dev]'

test:
	.venv/bin/python -m unittest discover -s tests -v

lint:
	.venv/bin/ruff check .

format-check:
	.venv/bin/ruff format --check .

run:
	.venv/bin/rdp-tui
