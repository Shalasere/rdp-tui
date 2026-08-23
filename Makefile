.PHONY: setup test run

setup:
	python -m venv .venv
	.venv/bin/python -m pip install --upgrade pip
	.venv/bin/python -m pip install -e .

test:
	.venv/bin/python -m unittest discover -s tests -v

run:
	./rdp-tui
