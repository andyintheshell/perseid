# HTTPS interception

Wiremoth can decrypt HTTPS by terminating TLS inside the proxy. It does this
with a certificate authority (CA) generated and stored locally: Wiremoth mints
a certificate for each host your browser connects to, signed by that CA.

Nothing is decrypted until you generate the CA, import its certificate into
your browser, and turn interception on. The CA private key never leaves your
machine.

Interception is off by default. With it off — or for hosts on the pass-through
list — `CONNECT` tunnels are relayed as opaque TLS, exactly as before.

## Where the CA lives

By default:

| File | Path |
| --- | --- |
| Certificate | `~/.local/share/wiremoth/wiremoth-ca.pem` |
| Private key | `~/.local/share/wiremoth/wiremoth-ca-key.pem` |

`$XDG_DATA_HOME` replaces `~/.local/share` when set. Override the directory with
`--ca-dir <DIR>`, or with the `WIREMOTH_CA_DIR` environment variable. The
directory is created with mode `0700`, the private key is written with mode
`0600`, and the public certificate with mode `0644`.

The certificate is safe to import into a browser. Do not share the private key:
anyone who has it can impersonate HTTPS sites to a client that trusts the CA.

## Generate the CA

Open **Proxy > TLS** and click **Generate certificate authority**. The panel
shows the certificate path and offers **Copy certificate path**, **Regenerate**,
and **Delete files**.

You can also inspect or remove the files directly:

```sh
ls -l "${WIREMOTH_CA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/wiremoth}"
```

Regenerating creates a new CA in place. Browsers that trusted the old
certificate will report an untrusted certificate until they import the new one.

## Trust the CA in a browser

Trust installation is always manual; Wiremoth never writes to a system or
browser trust store.

### Firefox

1. Open **Settings > Privacy & Security > Certificates > View Certificates**.
2. On the **Authorities** tab, click **Import** and select `wiremoth-ca.pem`.
3. Check **Trust this CA to identify websites**, then click **OK**.

### Chrome, Chromium, and other NSS-based Linux browsers

On Linux, Chrome and Chromium read user-added authorities from the NSS database
at `~/.pki/nssdb`. Install the `libnss3-tools` package if `certutil` is missing:

```sh
mkdir -p "$HOME/.pki/nssdb"
certutil -d sql:"$HOME/.pki/nssdb" -A -t "C,," -n "Wiremoth CA" \
  -i "$HOME/.local/share/wiremoth/wiremoth-ca.pem"
```

Restart the browser after importing. On macOS and Windows, Chrome uses the
system certificate store; import the certificate with the platform's certificate
manager, trusting it for SSL/TLS only.

> [!WARNING]
> A trusted CA can sign certificates for every site. Only trust it in a browser
> profile used for authorized testing, and remove the trust when you are done.

## Point the browser at Wiremoth and enable decryption

Configure the browser's HTTP and HTTPS proxy as `127.0.0.1:8080` (or the address
passed with `--listen`). Then:

1. In **Proxy > TLS**, check **Decrypt HTTPS (TLS interception)**.
2. Browse an HTTPS site. Each request and response appears as its own row in
   **Proxy > HTTP history**, with an `https://` URL.
3. The original `CONNECT` row remains in history marked **TLS intercepted**,
   with the number of decrypted requests served over that tunnel.

Held interception, scope enforcement, and match/replace rules all apply to
decrypted HTTPS messages exactly as they do to plain HTTP. Only HTTP/1.1 is
negotiated inside the tunnel; the proxy advertises `http/1.1` to the browser and
to the origin server.

Interception is a per-run setting. Wiremoth has no settings persistence yet, so
the CA survives restarts but decryption starts off each time. Pass
`--tls-intercept` to start with it enabled when a CA already exists.

## Pass-through hosts

Some hosts should never be decrypted: certificate-pinned applications, online
banking, or non-HTTP TLS protocols that happen to use `CONNECT`. Add them under
**Proxy > TLS > Pass-through hosts**, or start Wiremoth with one or more
`--tls-passthrough <HOST>` flags.

Matching is the same as the scope list: a host matches its exact name, its
subdomains, and any port. The list is an exclude list, so an empty list excludes
nothing. Pass-through applies to new tunnels; an established tunnel keeps the
mode it started with.

## Verify interception

Visit an HTTPS page after enabling decryption. In **HTTP history**:

- The URL starts with `https://`.
- **Raw** shows the request and response, including compressed bodies decoded for
  display.
- The **CONNECT** row for the same host is labeled **TLS intercepted**.

If the browser shows a certificate warning instead, see the troubleshooting
section below.

## Remove trust and delete the CA

Removal is deliberately manual. Do it in this order:

1. Turn off **Decrypt HTTPS** (or close Wiremoth).
2. Remove **Wiremoth CA** from the browser:
   - Firefox: **Settings > Privacy & Security > Certificates > View
     Certificates > Authorities**, select the Wiremoth entry, and click
     **Delete or Distrust**.
   - Chrome/Chromium on Linux:

     ```sh
     certutil -d sql:"$HOME/.pki/nssdb" -D -n "Wiremoth CA"
     ```

   - macOS and Windows: remove the certificate from the system certificate
     manager.
3. Delete the CA files with **Proxy > TLS > Delete files**, or:

   ```sh
   rm -f "${WIREMOTH_CA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/wiremoth}/wiremoth-ca.pem" \
        "${WIREMOTH_CA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/wiremoth}/wiremoth-ca-key.pem"
   ```

Deleting the files without removing browser trust leaves a trusted but unused
CA behind. Remove both.

## Troubleshooting

- **The browser rejects the certificate:** the CA is not trusted in that browser
  profile, the CA was regenerated after the import, or the host is on the
  pass-through list while the client trusts only Wiremoth's CA. Re-import the
  current `wiremoth-ca.pem`.
- **`NET::ERR_CERT_AUTHORITY_INVALID` in Chrome on Linux:** the NSS import did
  not apply to this profile. Check it with
  `certutil -d sql:"$HOME/.pki/nssdb" -L` and restart the browser.
- **Some sites fail while others work:** certificate pinning or a non-HTTP
  protocol. Add the host to the pass-through list.
- **HTTP/2 or HTTP/3 sites behave differently:** Wiremoth negotiates HTTP/1.1
  inside the tunnel and does not proxy QUIC. Disable HTTP/3/QUIC in the browser
  if a site insists on it.
- **No decrypted rows appear:** decryption is off, no CA is loaded, or the host
  is on the pass-through list. Check the status in **Proxy > TLS**.
- **Traffic is captured but not editable:** only bodies within
  `--intercept-limit` are buffered; larger or streaming bodies are header-only.

## Safety

- The CA private key stays in the directory above and is never uploaded.
- Wiremoth never installs trust automatically and never edits browser or system
  trust stores.
- Captured HTTPS traffic lives in memory only and follows the same count and byte
  bounds as HTTP. See [Safety and privacy](safety.md).
- Only intercept systems you own or are explicitly authorized to test.
