# perseid-proxy

The HTTP/1.1 and HTTP/2 interception proxy used by the Perseid desktop
workbench. It provides editable request/response interception, opt-in scope
enforcement, match/replace rules, bounded body capture, CONNECT passthrough,
immutable events over a bounded channel, and a direct HTTP/HTTPS Repeater client
for manual replay.

The proxy is a library first: the caller owns history and presentation. See
the [repository README](https://github.com/andyintheshell/perseid) for setup,
safety guidance, and the current feature set.

## License

MIT OR Apache-2.0
