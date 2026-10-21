# Perseid

The native desktop workbench for Perseid. The package contains both the
Burp-style desktop application and its embedded HTTP/1.1 and HTTP/2
interception proxy. The UI provides the Target/Proxy/Intruder/Repeater/Decoder
workspace, editable interception, editable Repeater tabs with direct replay,
scope and match/replace controls, bounded in-memory history, site map, and
message inspector.

The proxy is available to Rust callers as `perseid::proxy`, with editable
request/response interception, opt-in scope enforcement, match/replace rules,
bounded body capture, CONNECT passthrough, TLS interception with a local CA,
immutable events over a bounded channel, and a direct HTTP/HTTPS Repeater client
for manual replay.

```rust,no_run
fn main() -> Result<(), perseid::proxy::StartError> {
    let proxy = perseid::proxy::Proxy::start(Default::default())?;
    println!("listening on {}", proxy.local_addr());
    while let Some(event) = proxy.try_recv() {
        println!("{event:?}");
    }
    Ok(())
}
```

See the [repository README](https://github.com/andyintheshell/perseid) for
setup, safety guidance, and the current feature set.

## License

MIT OR Apache-2.0
