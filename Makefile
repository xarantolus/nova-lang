all: build debug test coverage

build:
	cargo build --release

debug:
	cargo build

fuzz:
	cargo fuzz run basic -- -fork=$$(nproc)

test: test-all test-no-default

test-all:
	cargo test --all-features

test-no-default:
	cargo test --no-default-features

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

.PHONY: fuzz coverage test test-all test-no-default build debug clean
