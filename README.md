# Fuzzing

Setup:
```
cargo install cargo-fuzz grcov
```

Run fuzzing:
```
cargo fuzz run  basic -- -fork=$(nproc)
```
