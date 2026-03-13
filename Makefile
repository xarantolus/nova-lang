all: build debug test coverage

build:
	cargo build --release

debug:
	cargo build

fuzz-basic:
	cargo fuzz run basic -- -fork=$$(nproc)

fuzz-reuse:
	cargo fuzz run reuse -- -fork=$$(nproc)

fuzz: fuzz-basic-half fuzz-reuse-half

fuzz-basic-half:
	cargo fuzz run basic -- -fork=$$(expr $$(nproc) / 2)

fuzz-reuse-half:
	cargo fuzz run reuse -- -fork=$$(expr $$(nproc) / 2

test: test-all test-no-default test-macros

test-all:
	cargo test --all-features

test-no-default:
	cargo test --no-default-features

test-macros:
	cargo test -p nova_macros --all-features

check-panic:
	cargo clippy -p nova --lib
	cargo build --release -p check-no-panic --target thumbv7em-none-eabihf

coverage:
	cargo fuzz coverage basic
	grcov . \
		-s . \
		--binary-path ./target/ \
		-t lcov \
		--branch \
		--ignore-not-existing \
		-o lcov.info

clean:
	cargo clean

.PHONY: fuzz coverage test test-all test-no-default test-macros check-panic build debug clean
