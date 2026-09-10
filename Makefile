test:
	cargo test --locked --all

lint:
	cargo clippy --all-targets -- -D warnings
	cargo fmt -- --check

bench:
	cargo bench --bench store_bench

.PHONY: test lint bench
