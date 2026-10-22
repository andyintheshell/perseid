# HTTPS interception

Perseid decrypts HTTPS by terminating TLS inside the proxy. It does this with a
certificate authority (CA) generated automatically on first launch and stored
locally: Perseid mints a certificate for each host your browser connects to,
signed by that CA.

HTTPS interception is on by default, as it is in Burp and ZAP. The CA is created
once and reused across restarts, development builds, and updates. On first
launch Perseid opens a short certificate setup window with a link to the local
download page. The only step Perseid cannot do for you is installing trust in
your browser; that stays deliberate and manual.

## Where the CA lives

| Platform | Default directory |
| --- | --- |
| Linux and other Unix-like systems | `$XDG_DATA_HOME/perseid`, or `~/.local/share/perseid` |
| macOS | `~/Library/Application Support/perseid` |
| Windows | `%LOCALAPPDATA%\perseid`, or `%APPDATA%\perseid` |

Each committed CA generation contains two files:

| File | Purpose |
| --- | --- |
| `perseid-ca.pem` | Public certificate; safe to import into a browser |
| `perseid-ca-key.pem` | Private key; never share or copy off the machine |

The directory is created with mode `0700`, the private key is written with mode
`0600`, and the public certificate with mode `0644`. On Windows the platform ACLs
apply instead.

New CAs live in a private `.perseid-ca-<generation>/` subdirectory. A small
`.perseid-ca-current` file selects the complete generation atomically; the
certificate and key are never replaced one at a time. **Proxy > Proxy settings**
shows the active paths. A `.perseid-ca.lock` file serializes readers, generation,
and deletion across processes. It contains no key material and may remain after
the CA is deleted.

Older installations with the two PEM files directly in the data directory are
still loaded unchanged. Explicit regeneration switches to the generation layout.
Failed staging leaves the previous pair intact; successful commits remove old
generations and legacy files. The loader rejects mismatched certificate/key pairs.

Override the directory with `--ca-dir <DIR>` or the `PERSEID_CA_DIR`
environment variable. This is useful for sandboxed runs, tests, and keeping an
experimental CA separate from the one your browser trusts.

## Created once, then reused

On startup Perseid loads the existing CA if both files are present. If the
directory is empty, it creates the CA before the proxy starts. It does not
replace an existing CA when the app updates, and debug and release builds use
the same directory, so `cargo run -p perseid` keeps the certificate you already
trusted.

The generated CA is long-lived and is never rotated automatically. If the files
are half-present, unreadable, or corrupt, Perseid reports the error in **Proxy >
TLS** and leaves HTTPS in passthrough instead of silently creating a new
authority that your browser would not trust.

> [!NOTE]
> **Upgrading from Wiremoth or frop?** The trust subject and data directory
> changed to `Perseid CA` and the Perseid data directory. A CA created at an
> older application's path is not reused; remove that trust entry, let Perseid
> create its own CA, and import the new certificate.

## Install the certificate

The easiest route is Perseid's built-in certificate page. With the proxy
running, open:

```text
http://127.0.0.1:8080/
```

or use **Proxy > Proxy settings > Open certificate page**. This works even before the
browser is configured to use the proxy, because the browser is connecting
directly to Perseid's listener. Once the proxy is configured you can also use:

```text
http://perseid/
```

Download `perseid-ca.pem` from the page and import it. Perseid serves only the
public certificate; the private key stays in the CA directory. Certificate-page
requests are handled by Perseid and do not appear in HTTP history.

### Firefox

1. Open **Settings > Privacy & Security > Certificates > View Certificates**.
2. On the **Authorities** tab, click **Import** and select `perseid-ca.pem`.
3. Check **Trust this CA to identify websites**, then click **OK**.
4. Restart Firefox.

### Chrome, Chromium, and other NSS-based Linux browsers

On Linux, Chrome and Chromium read user-added authorities from the NSS database
at `~/.pki/nssdb`. Install the `libnss3-tools` package if `certutil` is missing:

```sh
mkdir -p "$HOME/.pki/nssdb"
certutil -d sql:"$HOME/.pki/nssdb" -A -t "C,," -n "Perseid CA" \
  -i /path/to/downloaded/perseid-ca.pem
```

Use the downloaded certificate or the active certificate path shown in
**Proxy > Proxy settings**. Restart the browser after importing.

On macOS and Windows, Chrome uses the system certificate store; import the
certificate with the platform's certificate manager, trusting it for SSL/TLS
only.

> [!WARNING]
> A trusted CA can sign certificates for every site. Only trust it in a browser
> profile used for authorized testing, and remove the trust when you are done.

## Point the browser at Perseid

Configure the browser's HTTP and HTTPS proxy as `127.0.0.1:8080` (or the address
passed with `--listen`). Then browse an HTTPS site. Each request and response
appears as its own row in **Proxy > HTTP history**, with an `https://` URL. Like
Burp, Perseid treats the `CONNECT` tunnel as transport: it is not listed as a
separate history request. When interception is off or a host is passed through,
the encrypted traffic has no plaintext request to show, so it does not add
history rows either.

Held interception, scope enforcement, and match/replace rules all apply to
decrypted HTTPS messages exactly as they do to plain HTTP. Inside intercepted
tunnels the proxy advertises `h2` and `http/1.1` through ALPN, preferring HTTP/2
when the client supports it. The same preference is offered to HTTPS origins,
with HTTP/1.1 as the fallback on both sides.

Interception is on by default. Turn it off in **Proxy > Proxy settings** for the current
run, or start with:

```sh
perseid --no-tls-intercept
```

The CA is still created and available for installation when interception is
off. Decryption and the toggle are per-run settings; the CA itself persists.

## Pass-through hosts

Some hosts should never be decrypted: certificate-pinned applications, online
banking, or non-HTTP TLS protocols that happen to use `CONNECT`. Add them under
**Proxy > Proxy settings > Pass-through hosts**, or start Perseid with one or more
`--tls-passthrough <HOST>` flags.

Matching is the same as the scope list: a host matches its exact name, its
subdomains, and any port. The list is an exclude list, so an empty list excludes
nothing. Pass-through applies to new tunnels; an established tunnel keeps the
mode it started with.

## Verify interception

Visit an HTTPS page after installing the certificate. In **HTTP history**:

- The URL starts with `https://`.
- **Pretty** and **Raw** show the request and response, including compressed
  bodies decoded for display.
- There is no separate `CONNECT` row; the request index counts actual HTTP
  requests, as in Burp.

If the browser shows a certificate warning instead, see the troubleshooting
section below.

## Regenerate or remove the CA

**Proxy > Proxy settings > Regenerate** creates a new CA in place. Browsers that trusted the
old certificate will report an untrusted certificate until they import the new
one.

Removal is deliberate. Do it in this order:

1. Turn off **Intercept HTTPS** (or close Perseid).
2. Remove **Perseid CA** from the browser:
   - Firefox: **Settings > Privacy & Security > Certificates > View
     Certificates > Authorities**, select the Perseid entry, and click
     **Delete or Distrust**.
   - Chrome/Chromium on Linux:

     ```sh
     certutil -d sql:"$HOME/.pki/nssdb" -D -n "Perseid CA"
     ```

   - macOS and Windows: remove the certificate from the system certificate
     manager.
3. Delete the CA files with **Proxy > Proxy settings > Delete files**. This
   removes the current generation, any abandoned staging generations, the
   generation pointer, and legacy PEM files. Removing only the two legacy
   top-level PEM paths is not sufficient for the generation layout.

Deleting the files without removing browser trust leaves a trusted but unused
CA behind. Remove both. If the files are deleted, Perseid creates a fresh CA the
next time it starts; that new authority is not trusted until you import it.

## Troubleshooting

- **The browser rejects the certificate:** the CA is not trusted in that browser
  profile, the CA was regenerated after the import, or the host is on the
  pass-through list while the client trusts only Perseid's CA. Re-import the
  current `perseid-ca.pem`.
- **`NET::ERR_CERT_AUTHORITY_INVALID` in Chrome on Linux:** the NSS import did
  not apply to this profile. Check it with
  `certutil -d sql:"$HOME/.pki/nssdb" -L` and restart the browser.
- **The certificate page does not open:** the proxy may not be running, or the
  listener is on a different address or port. Use the URL shown in **Proxy >
  TLS**, or start the proxy from the toolbar.
- **Some sites fail while others work:** certificate pinning or a non-HTTP
  protocol. Add the host to the pass-through list.
- **HTTP/3 sites behave differently:** Perseid proxies HTTP/1.1 and HTTP/2 but
  does not proxy QUIC. Disable HTTP/3/QUIC in the browser if a site insists on
  it.
- **No decrypted rows appear:** interception is off, no CA is loaded, or the
  host is on the pass-through list. Check the status in **Proxy > Proxy settings**.
- **Traffic is captured but not editable:** only bodies within
  `--intercept-limit` are buffered; larger or streaming bodies are header-only.
- **The CA could not be created at startup:** Perseid shows the error under
  **Proxy > Proxy settings**. Check that the data directory is writable, or point
  `--ca-dir` at a writable location, then retry.

## Safety

- The CA private key stays in the directory above and is never uploaded.
- Perseid creates the CA automatically, but never installs it into a system or
  browser trust store. Trust and removal are always user actions.
- Captured HTTPS traffic lives in memory only and follows the same count and
  byte bounds as HTTP. See [Safety and privacy](safety.md).
- Only intercept systems you own or are explicitly authorized to test.
