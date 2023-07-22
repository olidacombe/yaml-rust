all::
	cargo build

install:: all
	@echo info: Nothing to install. This package does not provide binaries.

test::
	cargo test

check::
	cargo clippy --all -- -D warnings

clean::
	cargo clean

fix::
	cargo clippy --allow-dirty --allow-staged --fix --all -- -D warnings
