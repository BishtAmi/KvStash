# kvstash

`kvstash` is a learning project for building a prefix-aware KV cache service in
Rust. It currently includes content-addressed chunk keys, an in-memory
byte-budgeted LRU, a bounded binary protocol, a loopback TCP server, and a
sequential async Rust client.

## Development

Run the workspace tests:

```sh
cargo test --workspace
```

See [the concepts guide](docs/concepts.md) for how chained prefix keys work and
[the implementation approach](docs/implementation-plan.md) for the staged
roadmap and design review. The [Rust project guide](docs/rust-project-guide.md)
explains the workspace layout and Rust implementation concepts. See the
[architecture diagram](docs/architecture.md) for component responsibilities
and request flow.

## Run the demo

In one terminal:

```sh
cargo run -p kv-server
```

In another:

```sh
cargo run -p kv-client --example demo
```

The server binds to `127.0.0.1:7070` and defaults to a 1 GiB payload budget.
Override the budget with `--capacity-bytes N`. The client deliberately sends
one request at a time per connection; request IDs are present on the wire, but
concurrent in-flight request multiplexing is not implemented yet.
