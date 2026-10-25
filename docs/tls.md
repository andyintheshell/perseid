# HTTPS interception

[Documentation](README.md) · [Getting started](getting-started.md) ·
[Workbench](workbench.md) · [Troubleshooting](troubleshooting.md)

Perseid creates a local certificate authority (CA) on first launch and uses it
to issue certificates for intercepted hosts. HTTPS decryption is enabled by
default in the **desktop app**, but **trust is never installed automatically**.

> [!WARNING]
> Trusting a CA allows it to authenticate sites to the trusting client. Prefer
> a dedicated testing browser profile, keep the private key local, and remove
> trust when you are finished. Only intercept explicitly authorized targets.

## Install the certificate

1. Start Perseid and check the toolbar shows its proxy listening.
2. Open `http://127.0.0.1:8080/` **directly**, or choose **Proxy > Proxy
   settings > Open certificate page**. Use the actual listen port if changed.
3. Download **`perseid-ca.pem`**, the public certificate—not the private key.
4. Import it using the instructions for your testing browser below.
5. Configure an **HTTP proxy** at `127.0.0.1:8080` for both HTTP and HTTPS.
   This is an HTTP proxy carrying HTTPS `CONNECT` tunnels, not a SOCKS proxy.

The certificate page works before the browser is configured to proxy, because
it is served by the local listener. Once proxying is configured,
`http://perseid/` opens the same page. Certificate-page requests are not added
to captured HTTP history.

### Firefox

In the dedicated testing profile:

1. Open **Settings > Privacy & Security > Certificates > View Certificates**.
2. On **Authorities**, choose **Import** and select the downloaded public CA.
3. Enable trust for identifying websites and confirm.
4. Restart the browser if needed, then visit your authorized HTTPS target.

Menu labels vary by Firefox release. Importing the CA in one profile does not
mean it has been imported in every other profile.

### Chrome and Chromium on Linux

NSS-based Linux setups commonly use `~/.pki/nssdb`. Install `libnss3-tools`
with your distribution's package manager if `certutil` is missing, then import
the **downloaded public certificate**:

```sh
mkdir -p "$HOME/.pki/nssdb"
certutil -d sql:"$HOME/.pki/nssdb" -A -t "C,," -n "Perseid CA" \
  -i /path/to/downloaded/perseid-ca.pem
```

Replace the placeholder path with the certificate you downloaded. You can also
use the certificate path shown in **Proxy > Proxy settings**. Certificate-store
behavior can vary by browser build or packaging; use its certificate manager
if it does not use that NSS database. Restart the browser after importing.

### Chrome and Chromium on macOS

These browsers use OS certificate trust. You can import the public CA through
**Keychain Access > File > Import Items**, typically into your **login**
keychain. Open the imported **Perseid CA**, expand **Trust**, and explicitly
trust it for **Secure Sockets Layer (SSL)**. macOS may ask you to confirm with
your account credentials. Restart the browser if needed.

**This can affect other applications using that keychain, not just one browser
profile.** Use Firefox's profile-specific authority import instead when you
want to keep testing trust separate from your normal OS/browser trust. Do not
import `perseid-ca-key.pem` and do not install anything into the System keychain
just to follow this guide.

### Other browser/platform combinations

Use the browser's documented certificate manager and trust only the public CA
for website/SSL authentication. Windows builds and tests run in CI. Clean-machine
release installs and manual browser trust workflows still need broader smoke
testing; see
[platform status](README.md#platform-status).

## Verify interception

Visit an HTTPS endpoint on your local lab or another authorized target. In
**Proxy > HTTP history**, you should see individual `https://` requests and
their decrypted responses. There is no separate `CONNECT` row; that tunnel is
transport, not an HTTP exchange shown in the history UI.

To test a client without changing its trust store, pass the public CA to curl
for that request only, replacing both placeholders:

```sh
curl --noproxy "" --proxy http://127.0.0.1:8080 \
  --cacert /path/to/downloaded/perseid-ca.pem \
  https://your-authorized-lab.test/
```

Do not substitute `--insecure` for configuring trust. The browser/client trusting
Perseid and Perseid trusting the upstream server are **separate** TLS checks.
Origins are checked against platform trust anchors, with bundled public roots
as a fallback when no native roots are available.

The proxy negotiates HTTP/2 or HTTP/1.1 with intercepted clients and HTTPS
origins. Decrypted requests follow the normal interception, scope, and rewrite
pipeline. **Intercept HTTPS** controls decryption; **Intercept is off/on**
controls whether messages wait for your Forward/Drop decision.

## Where the CA lives

| Platform | Default CA data directory |
| --- | --- |
| Linux and other Unix-like systems | `$XDG_DATA_HOME/perseid`, or `~/.local/share/perseid` |
| macOS | `~/Library/Application Support/perseid` |
| Windows | `%LOCALAPPDATA%\perseid`, falling back to `%APPDATA%\perseid` |

`PERSEID_CA_DIR` overrides the platform location; `--ca-dir <DIR>` overrides
both. **Proxy > Proxy settings** shows the active certificate and key paths.
Newly created or explicitly regenerated CAs use this layout:

```text
perseid/
├── .perseid-ca-current
├── .perseid-ca.lock
└── .perseid-ca-<generation>/
    ├── perseid-ca.pem
    └── perseid-ca-key.pem
```

| File | Purpose |
| --- | --- |
| `perseid-ca.pem` | Public certificate inside the active generation; safe to import into a testing client. |
| `perseid-ca-key.pem` | Private key inside that generation; never share, upload, or copy off the machine. |
| `.perseid-ca-current` | A small pointer selecting the complete active generation atomically. |
| `.perseid-ca.lock` | Cross-process coordination for loading, generation, and deletion; contains no key material. |

The data and generation directories use mode `0700`, keys use `0600`, and public
certificates use `0644` on Unix. The pointer and lock use `0600`. Windows uses
its platform ACL behavior instead. Use the paths in the UI or download the
public certificate; do not assume the PEM files are directly in the data root.
Do not edit the pointer or unlink the lock file to repair a trust problem.

Both PEM documents are staged, synced, and checked for a matching key before
the active pointer is replaced. A failure before commit leaves the previous CA
active. Store operations are serialized across processes so a loader does not
read half of a replacement pair. Successful commits normally remove old
generations and legacy PEM files; cleanup warnings can leave superseded files
for a later cleanup. The lock file may remain after deletion and is not a CA.

### Existing installations and upgrades

Existing CAs with both PEM files directly in the data directory are **reused
unchanged**. Merely updating, rebuilding, or restarting the app does not rotate
them or move them into a new generation. An explicit **Regenerate** creates a
new CA using the generation layout and requires importing the new certificate.

The CA is otherwise reused across debug/release builds and restarts. An
incomplete store, invalid generation pointer, or mismatched certificate/key pair
is reported rather than silently replaced. If loading fails, HTTPS stays in
pass-through until a CA is available. Use an isolated `--ca-dir` for development
experiments.

The headless example and `ProxyConfig::default()` **do not** automatically
create a CA or enable decryption. Those are desktop-app startup decisions;
see the [library guide](../crates/perseid/README.md).

If upgrading from an older project name such as Wiremoth or frop, the old data
directory and trust entry are not automatically migrated. Remove the old trust
entry and explicitly import the certificate created by Perseid.

## Pass-through hosts

Certificate-pinned applications and non-HTTP TLS protocols may need opaque
tunnels. Add hosts in **Proxy > Proxy settings > Pass-through hosts**, or:

```sh
cargo run -p perseid --locked -- --tls-passthrough pinned.lab.test
```

Each entry matches the host, its subdomains, and any port. An empty pass-through
list excludes nothing. Scope can still block the tunnel target, but opaque
tunnel contents cannot be inspected, rewritten, or listed as decrypted history.

To keep all HTTPS opaque for a run:

```sh
cargo run -p perseid --locked -- --no-tls-intercept
```

The CA is still prepared. Turning off decryption does not delete the key or
remove trust. Mode changes and pass-through rules apply to new tunnels;
existing tunnels keep the mode they started with.

## Regenerate or remove the CA

**Regenerate** in **Proxy > Proxy settings** intentionally creates a new CA.
Browsers that trusted the old certificate must import the replacement; remove
the old trust entry as well. Regeneration is not the first remedy for a browser
proxy-setting problem.

To remove the CA safely:

1. Turn off HTTPS interception or close Perseid.
2. Remove **Perseid CA** from every client trust store where you installed it:
   - Firefox: select it under **Authorities** and choose **Delete or Distrust**.
   - NSS-based Linux setups:

     ```sh
     certutil -d sql:"$HOME/.pki/nssdb" -D -n "Perseid CA"
     ```

   - macOS: remove the certificate you imported in Keychain Access.
   - Other clients: use the same certificate manager used for import.
3. Use **Proxy > Proxy settings > Delete files**. It removes the active and
   abandoned generations, the generation pointer, and legacy PEM files. Deleting
   only the two old top-level PEM paths is **not sufficient** for the generation
   layout. The coordination lock may remain; it contains no key material.

Deleting files does not remove browser/OS trust. Turning interception off does
neither. On the next launch, Perseid creates a new CA if the store is empty;
that new authority is not trusted until you explicitly import it.

## Troubleshooting

| Symptom | Check |
| --- | --- |
| Browser rejects the certificate | The CA must be trusted in that client/profile. If regenerated, import the current public certificate. |
| Certificate page does not open | Check the listener and actual port. Open its loopback URL directly before configuring the browser proxy. |
| No decrypted history rows | Check **Intercept HTTPS**, CA readiness, client proxy settings, and the pass-through list. |
| Only some hosts fail | Certificate pinning, private upstream CAs, or non-HTTP TLS may be involved; consider intentional pass-through. |
| CA creation or loading fails | Check the error, store path, and permissions. Do not mix PEMs from different generations. A dedicated writable `--ca-dir` isolates experiments without altering the usual CA. |
| PEM files seem missing | New stores use a hidden generation subdirectory. Use the active paths in **Proxy > Proxy settings** or the certificate download page. |
| `NET::ERR_CERT_AUTHORITY_INVALID` on Linux | Check the certificate manager or `certutil -d sql:"$HOME/.pki/nssdb" -L`, and restart the browser. |
| HTTP/3 behaves differently | QUIC is not proxied; use a client/target that can fall back to HTTP/1.1 or HTTP/2. |

See [Troubleshooting](troubleshooting.md) for held requests, missing history,
editing limits, and safe bug-report diagnostics. Read
[Safety and privacy](safety.md) before intercepting real traffic.
