# Miden CLOB Design Benchmarking

this repo tests and benchmarks various CLOB designs on Miden

running basic swapp test:
```bash
cargo test --release  swap_note_partial_consume_test -- --nocapture
```

running optimistic consumption swapp test:
```bash
cargo test --release  partial_swap_ephemeral_chain_benchmark -- --nocapture
```

running super fast signature check test:
```bash
cargo test --release multi_signature_benchmark_advice_provider -- --exact --nocapture
```

running super fast signature check test with 100 signatures:
```bash
cargo test --release multi_signature_benchmark_advice_provider_100 -- --exact --nocapture
```

Formatting masm:
```
cargo masm-fmt masm
```