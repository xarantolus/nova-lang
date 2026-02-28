coverage:
	cargo fuzz coverage basic
	grcov . \
		-s . \
		--binary-path ./target/ \
		-t lcov \
		--branch \
		--ignore-not-existing \
		-o lcov.info
