.PHONY: format lint features docs test throughput msrv workshop package deny outdated quality

format:
	cargo fmt --all -- --check

lint:
	cargo clippy --workspace --all-targets --all-features -- -D warnings

features:
	cargo hack clippy -p autotel --each-feature --all-targets -- -D warnings

docs:
	RUSTDOCFLAGS="-D warnings" cargo doc -p autotel --no-deps --all-features

test:
	cargo test --workspace --all-targets --all-features
	cargo test -p autotel --doc --all-features
	cargo test -p autotel --all-targets

throughput:
	cargo test --release -p autotel --test throughput -- --ignored --nocapture

msrv:
	cargo +1.88.0 check -p autotel --lib --all-features
	cargo +1.88.0 check -p autotel --lib --no-default-features

workshop:
	cargo workshop-check

package:
	cargo package -p autotel --allow-dirty

deny:
	cargo deny check

outdated:
	cargo outdated --root-deps-only --exit-code 1

quality: format lint features docs test msrv throughput workshop package deny
