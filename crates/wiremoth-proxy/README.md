# wiremoth-proxy

The HTTP/1.1 interception proxy used by the Wiremoth desktop workbench. It
provides bounded body capture, CONNECT passthrough, and immutable events over
a bounded channel.

The proxy is a library first: the caller owns history and presentation. See
the [repository README](https://github.com/andyintheshell/wiremoth) for setup,
safety guidance, and the current feature set.

## License

MIT OR Apache-2.0
