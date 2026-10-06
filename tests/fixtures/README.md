# Test fixtures

`minimal.ttf` is copied verbatim from
[`font-parse`](https://github.com/WyattAu/font-parse)'s `tests/fixtures/`
(the crate's own known-good fixture font, public domain / MIT). It is here so
the `inventory` example can be run against a real file without a network
fetch:

```sh
cargo run --example inventory -- tests/fixtures/minimal.ttf
```

`font-parse` is a direct dependency, so a future version of that fixture can be
re-copied without a version bump here.
