# Runbook: `curl` exit 60 / TLS hostname (SNI) mismatch

## Meaning

`curl: (60) SSL: no alternative certificate subject name matches target host name 'example.com'` means the certificate presented on TLS **does not list** that hostname in **CN or SAN**, or a **different** `server` block / default certificate is answering for the SNI you tested.

This is **not** a generic “connection failed”; it is a **name mismatch** between probe host and certificate.

## Quick checks on the host

1. **SNI and presented cert** (replace host and path to fullchain):

   ```bash
   echo | openssl s_client -servername example.com -connect 127.0.0.1:443 2>/dev/null \
     | openssl x509 -noout -subject -ext subjectAltName
   ```

2. **Declared nginx `server_name`** for the vhost that should serve HTTPS:

   ```bash
   grep -R "server_name\|ssl_certificate" /etc/nginx/sites-available/ /etc/nginx/conf.d/ 2>/dev/null
   ```

3. **On-disk cert vs hostname**:

   ```bash
   sudo openssl x509 -in /etc/letsencrypt/live/<name>/fullchain.pem -noout -checkhost example.com
   ```

   Exit `0` and “matches” means the PEM is valid for that host.

4. **Duplicate `server_name`** across vhosts: the wrong server block may win; use control-api nginx inventory / `GET` preflight conflicts.

## Pirate stack behavior (after fixes)

- HTTPS probes classify this as **`tls_name_mismatch`** (not `connect_failed`).
- **`set_ssl`**: optional **openssl `-checkhost` preflight** before applying nginx; probe uses **retries** after reload; **rollback is skipped** for `tls_name_mismatch` so a previous config is not restored when the issue is cert/name alignment (fix cert or `server_name` / `post_check_host`).
- **gRPC SSL post-check**: set **`SSL_POST_CHECK_HOST`** if the public UI/control hostname must differ from DB primary / domain list order.

## Related env

- `SSL_POST_CHECK_HOST` — SNI hostname for deploy-server HTTPS smoke probe after renew.
- `SSL_POST_CHECK_PATH`, `SSL_POST_CHECK_PORT`, `SSL_POST_CHECK_LOOPBACK` — probe URL and `--resolve` target.
